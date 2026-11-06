#!/usr/bin/env python3
"""Linux storage faults in a private mount namespace; never fill a host disk.

Run via unshare --user --map-root-user --mount --propagation private, or sudo
unshare --mount --propagation private. The only mounted filesystem is a new
64KiB tmpfs. strace faults target one exact owned log pathname.
"""
import argparse
import json
import os
from pathlib import Path
import signal
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


def traced(command, retained_state=None):
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               start_new_session=True)
    try:
        if retained_state is not None:
            deadline = time.monotonic() + 10
            while not (retained_state / "result.json").exists():
                assert process.poll() is None, "tracer exited without retained completion"
                assert time.monotonic() < deadline, "stalled log prevented bounded result publication"
                time.sleep(0.01)
        # A traced thread's exit can itself wait for strace's injected timer.
        # Publication above, not tracer exit, measures supervisor responsiveness.
        output, errors = process.communicate(timeout=25 if retained_state is not None else 12)
        assert process.returncode == 0, (output, errors)
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=5)


def failed_publication(state, marker, trace=None):
    parent, initial = socket.socketpair()
    parent.settimeout(10)
    command = [str(monitor), "--state-dir", str(state), "--control", str(initial.fileno()),
               "--log-path", "none", "--exit-command", str(fixture)]
    for argument in ["touch", str(marker)]:
        command += ["--exit-command-arg", argument]
    command += ["--", str(fixture), "emit"]
    if trace is not None:
        # The exact directory path targets the post-rename directory fsync,
        # leaving the temporary result file's synchronization untouched.
        command = ["strace", "-f", "-e", "trace=fsync", "-e", "inject=fsync:error=EIO",
                   "-P", str(state), "-o", str(trace)] + command
    process = subprocess.Popen(command, pass_fds=(initial.fileno(),),
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               start_new_session=True)
    initial.close()
    try:
        output, errors = process.communicate(timeout=10)
        assert process.returncode != 0, (output, errors)
        expected = b"Input/output error" if trace is not None else b"No space left"
        assert expected in errors, errors
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
        if trace is not None:
            assert "(INJECTED)" in trace.read_text(), "directory fsync fault was not exercised"
            # Rename has already published valid facts, but failed directory
            # synchronization must not acknowledge durable completion or run
            # the exit hook. Preserve the known leader status on disk.
            assert result(state)["leader"] == {"type": "exit", "code": 37}
        else:
            assert not (state / "result.json").exists(), "failed write published a result"
        assert not marker.exists(), "exit command ran after failed publication"
    finally:
        parent.close()
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=5)


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
        traced(command)
        retained = result(state)
        assert retained["leader"] == {"type": "exit", "code": 37}, retained
        if no_sync:
            assert not retained["log_errors"], retained
            assert "(INJECTED)" not in trace.read_text(), trace.read_text()
        else:
            assert any("log synchronization" in error for error in retained["log_errors"]), retained
            assert "(INJECTED)" in trace.read_text(), "the fsync fault was not exercised"

    for syscall, mode, cause in [("write", "flood", "timed_out"),
                                 ("fsync", "emit", "leader_exit")]:
        state = root / ("blocked-" + syscall)
        trace = root / (state.name + ".trace")
        command = ["strace", "-f", "-e", f"trace={syscall}",
                   "-e", f"inject={syscall}:delay_exit=20s:when=1",
                   "-P", str(state / "console.log"), "-o", str(trace),
                   str(monitor), "--state-dir", str(state), "--timeout", "1",
                   "--", str(fixture), mode]
        traced(command, state)
        assert "(DELAYED)" in trace.read_text(), f"the {syscall} stall was not exercised"
        retained = result(state)
        assert retained["cause"] == cause, retained
        assert retained["cleanup"] == {"type": "complete"}, retained
        assert retained["output_truncated"], retained
        assert any("output drain/log synchronization did not complete" in error
                   for error in retained["log_errors"]), retained
        if mode == "emit":
            assert retained["leader"] == {"type": "exit", "code": 37}, retained

    failed_publication(root / "directory-sync-state", root / "no-sync-callback",
                       root / "directory-sync.trace")

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
        failed_publication(full / "publication-state", root / "must-not-run")
    finally:
        subprocess.run(["umount", str(full)], check=True, timeout=5)
print("Storage fault qualification passed: fsync error/opt-out, bounded blocked write/fsync and full disk, failed publication without final acknowledgement or callback")
print("Directory synchronization failure: known result preserved; no completion acknowledgement or exit callback")
