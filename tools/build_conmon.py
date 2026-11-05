"""Build the pinned C conmon test oracle; never used by the product."""
import hashlib
import json
import pathlib
import subprocess
import sys
import tarfile
import urllib.request

REVISION = "46837cd4800889521b6eccd4cce75a69249b1702"
ARCHIVE_SHA256 = "dd871d18a4a81898e46778cce4453e5afedcd24ec3708dbf7182764297ac014d"


def main():
    destination = pathlib.Path(sys.argv[1]).resolve()
    destination.mkdir(parents=True, exist_ok=True)
    archive = destination / "conmon.tar.gz"
    if not archive.exists():
        urllib.request.urlretrieve(
            f"https://codeload.github.com/containers/conmon/tar.gz/{REVISION}", archive
        )
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    if digest != ARCHIVE_SHA256:
        raise RuntimeError("conmon source archive does not match the pinned digest")
    source = destination / f"conmon-{REVISION}"
    if source.exists():
        raise RuntimeError("refusing to reuse an existing oracle source tree")
    with tarfile.open(archive) as package:
        package.extractall(destination, filter="data")
    subprocess.run(["make", "-j4"], cwd=source, check=True)
    binary = source / "bin/conmon"
    (destination / "oracle.json").write_text(json.dumps({
        "revision": REVISION,
        "archive_sha256": digest,
        "binary": str(binary),
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
    }, indent=2) + "\n")


if __name__ == "__main__":
    main()
