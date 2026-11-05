"""Differential CRI/exit fixture. Not a full C01-C30 conformance pass."""
import hashlib
import json
import pathlib
import re
import subprocess
import sys
import tempfile
from build_conmon import REVISION, ARCHIVE_SHA256

TIMESTAMP = re.compile(rb"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{9}[+-]\d\d:\d\d")


def decode(path):
    records = []
    # CRI separates records only with LF; other binary control bytes are data.
    for record in path.read_bytes().split(b"\n")[:-1]:
        line = record + b"\n"
        time, stream, kind, payload = line.split(b" ", 3)
        if not TIMESTAMP.fullmatch(time) or stream not in (b"stdout", b"stderr"):
            raise AssertionError(f"invalid timestamp/stream: {line!r}")
        if kind not in (b"F", b"P") or not payload.endswith(b"\n"):
            raise AssertionError(f"invalid CRI record: {line!r}")
        records.append((stream, kind, payload))
    # stdout/stderr are separate pipes; their relative read order is not fixed.
    # Preserve every byte, record boundary, F/P marker and per-stream order.
    return {stream: [(kind, data) for source, kind, data in records if source == stream]
            for stream in (b"stdout", b"stderr")}


def main():
    manifest = json.loads(pathlib.Path(sys.argv[1]).read_text())
    if manifest["revision"] != REVISION or manifest["archive_sha256"] != ARCHIVE_SHA256:
        raise AssertionError("wrong conmon reference revision")
    oracle = pathlib.Path(manifest["binary"])
    if hashlib.sha256(oracle.read_bytes()).hexdigest() != manifest["binary_sha256"]:
        raise AssertionError("oracle binary changed after its build")
    madrillo = pathlib.Path(sys.argv[2]).resolve()
    fixture = pathlib.Path(sys.argv[3]).resolve()
    for iteration in range(12):
        with tempfile.TemporaryDirectory(prefix="madrillo-oracle-") as temp:
            root = pathlib.Path(temp)
            reference = root / "reference"
            reference.mkdir()
            result = subprocess.run([
                str(oracle), "--sync", "--cid", "madrillo-fixture", "--cuuid", "madrillo-fixture",
                "--runtime", str(fixture), "--bundle", str(reference),
                "--container-pidfile", str(reference / "pid"), "--persist-dir", str(reference),
                "--socket-dir-path", str(reference), "--log-path", str(reference / "console.log"),
            ], capture_output=True, timeout=10)
            if result.returncode != 37 or (reference / "exit").read_bytes() != b"37":
                raise AssertionError(f"oracle fixture failed: {result!r}")
            actual = root / "actual"
            subprocess.run([str(madrillo), "--state-dir", str(actual), "--", str(fixture), "emit"],
                           check=True, capture_output=True, timeout=10)
            completion = json.loads((actual / "result.json").read_text())
            if completion["leader"] != {"type": "exit", "code": 37}:
                raise AssertionError(completion)
            expected = decode(reference / "console.log")
            observed = decode(actual / "console.log")
            if observed != expected:
                raise AssertionError(f"iteration {iteration}: {observed!r} != {expected!r}")
    print(f"12 CRI/binary/native-exit comparisons passed against conmon {REVISION}")


if __name__ == "__main__":
    main()
