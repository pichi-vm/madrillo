#!/usr/bin/env python3
"""Compare actual journald records and combined file output against pinned conmon."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import uuid
from build_conmon import REVISION, ARCHIVE_SHA256
from compare_conmon import decode

identity = json.loads(Path(sys.argv[1]).read_text())
assert identity["revision"] == REVISION and identity["archive_sha256"] == ARCHIVE_SHA256
oracle = Path(identity["binary"])
assert hashlib.sha256(oracle.read_bytes()).hexdigest() == identity["binary_sha256"]
monitor, fixture = (str(Path(p).resolve()) for p in sys.argv[2:4])


def records(identifier, count):
    deadline = time.monotonic() + 5
    while True:
        query = subprocess.run(["journalctl", "--no-pager", "-o", "json", "--since", "-1min",
                                f"CONTAINER_ID_FULL={identifier}"], capture_output=True, check=True, timeout=10)
        entries = [json.loads(line) for line in query.stdout.splitlines()]
        if len(entries) >= count:
            break
        if time.monotonic() >= deadline:
            raise AssertionError(f"missing journal records for {identifier}: {entries!r}; {query.stderr!r}")
        time.sleep(0.05)
    # Ignore journal attribution/time/cursor, and the intentionally distinct IDs.
    # Preserve messages, priority, partial marker, identity metadata and labels.
    keys = ["MESSAGE", "PRIORITY", "CONTAINER_PARTIAL_MESSAGE", "CONTAINER_ID",
            "CONTAINER_NAME", "CONTAINER_TAG", "SYSLOG_IDENTIFIER", "MADRILLO_TEST"]
    selected = [{key: entry.get(key) for key in keys} for entry in entries]
    # Separate stdout/stderr pipes have no defined relative read order.
    return sorted(selected, key=lambda entry: json.dumps(entry, sort_keys=True))


for mode, no_partial, count in [("emit", False, 4), ("emit", True, 4), ("journal-emit", False, 5)]:
    with tempfile.TemporaryDirectory(prefix="madrillo-journal-") as tmp:
        root = Path(tmp)
        reference = root / "reference"
        reference.mkdir()
        actual = root / "actual"
        prefix = uuid.uuid4().hex
        ref_id, actual_id = prefix + "reference", prefix + "actual"
        flags = ["--no-container-partial-message"] if no_partial else []
        conmon = subprocess.run([
            str(oracle), "--sync", "--cid", ref_id, "--cuuid", ref_id,
            "--runtime", fixture, "--bundle", str(reference),
            "--container-pidfile", str(reference / "pid"), "--persist-dir", str(reference),
            "--socket-dir-path", str(reference), "--log-path", "journald",
            "--log-path", f"k8s-file:{reference / 'console.log'}", "--name", "mother-armadillo",
            "--log-tag", "madrillo-qualification", "--log-label", "MADRILLO_TEST=binary", *flags,
        ], capture_output=True, timeout=10, env={**os.environ, "MADRILLO_FIXTURE_MODE": mode})
        assert conmon.returncode == 37, conmon.stderr
        spawned = subprocess.run([
            monitor, "--state-dir", str(actual), "--log-path", "journald",
            "--log-path", f"k8s-file:{actual / 'console.log'}", "--log-id", actual_id,
            "--log-name", "mother-armadillo", "--log-tag", "madrillo-qualification",
            "--log-label", "MADRILLO_TEST=binary", *flags, "--", fixture, mode,
        ], capture_output=True, timeout=10)
        assert spawned.returncode == 0, spawned.stderr
        completion = json.loads((actual / "result.json").read_text())
        assert not completion["log_errors"], completion
        expected, observed = records(ref_id, count), records(actual_id, count)
        assert expected == observed, (expected, observed)
        assert decode(reference / "console.log") == decode(actual / "console.log")
        # Exercise binary payloads explicitly so a lossy UTF-8 conversion fails.
        assert any(isinstance(entry["MESSAGE"], list) and 255 in entry["MESSAGE"] for entry in observed)
print("journald differential: binary payloads, metadata, priority, partial opt-out and combined file output passed")
