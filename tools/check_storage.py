#!/usr/bin/env python3
"""Linux storage faults in a private mount namespace; never fill a host disk.

Run via unshare --user --map-root-user --mount --propagation private, or sudo
unshare --mount --propagation private. The only mounted filesystem is a new
64KiB tmpfs. strace faults target one exact owned log pathname.
"""
import argparse
import json
from pathlib import Path
import socket
import struct
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument("monitor", type=Path)
parser.add_argument("fixture", type=Path)
args = parser.parse_args()
monitor, fixture = args.monitor.resolve(), args.fixture.resolve()


def result(state):
    return json.loads((state / "result.json").read_bytes())


with tempfile.TemporaryDirectory(prefix="madrillo-storage-") as temporary:
    root = Path(temporary)
    for no_sync in (False, True):
        state = root / ("no-sync" if no_sync else "sync")
        trace = root / (state.name + ".trace")
        command = ["strace", "-f", "-e", "trace=fsync", "-e", "inject=fsync:error=EIO",
                   "-P", str(state / "console.log"), "-o", str(trace),
                   str(monitor), "--state-dir", str(state)]
        if no_sync:
            command.append("--no-sync-log")
        command += ["--", str(fixture), "emit"]
        subprocess.run(command, check=True, capture_output=True, timeout=10)
        retained = result(state)
        assert retained["leader"] == {"type": "exit", "code": 37}, retained
        if no_sync:
            assert not retained["log_errors"], retained
            assert "(INJECTED)" not in trace.read_text(), trace.read_text()
        else:
            assert any("log synchronization" in error for error in retained["log_errors"]), retained
            assert "(INJECTED)" in trace.read_text(), "the fsync fault was not exercised"

    full = root / "full"
    full.mkdir()
    subprocess.run(["mount", "-t", "tmpfs", "-o", "size=64k,mode=0700", "madrillo-fault", str(full)], check=True)
    try:
        state = root / "disk-full-state"
        started = time.monotonic()
        subprocess.run([str(monitor), "--state-dir", str(state), "--log-path", str(full / "console.log"),
                        "--timeout", "1", "--", str(fixture), "flood"], check=True, capture_output=True, timeout=10)
        assert time.monotonic() - started < 5, "full logging sink stalled deadline handling"
        retained = result(state)
        assert retained["cause"] == "timed_out", retained
        assert any("No space left" in error for error in retained["log_errors"]), retained

        # The full filesystem can still allocate an empty directory/inode but
        # cannot publish the result data. No final acknowledgement or callback
        # may claim completion when retained publication fails.
        state = full / "publication-state"
        marker = root / "must-not-run"
        parent, initial = socket.socketpair()
        parent.settimeout(10)
        command = [str(monitor), "--state-dir", str(state), "--control", str(initial.fileno()),
                   "--log-path", "none", "--exit-command", str(fixture)]
        for argument in ["touch", str(marker)]:
            command += ["--exit-command-arg", argument]
        command += ["--", str(fixture), "emit"]
        process = subprocess.Popen(command, pass_fds=(initial.fileno(),), stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        initial.close()
        try:
            output, errors = process.communicate(timeout=10)
            assert process.returncode != 0, (output, errors)
            assert b"No space left" in errors, errors
            packets = b""
            while True:
                packet = parent.recv(65536)
                if not packet:
                    break
                packets += packet
            while len(packets) >= 4:
                length = struct.unpack(">I", packets[:4])[0]
                if len(packets) < length + 4:
                    break
                message = json.loads(packets[4:length + 4])
                assert message["message"]["type"] != "completed", message
                packets = packets[length + 4:]
            assert not (state / "result.json").exists(), "failed publication left a successful-looking result"
            assert not marker.exists(), "exit command ran after failed publication"
        finally:
            parent.close()
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
    finally:
        subprocess.run(["umount", str(full)], check=True, timeout=5)
print("Storage fault qualification passed: fsync error/opt-out, bounded full disk, failed publication without final acknowledgement or callback")
