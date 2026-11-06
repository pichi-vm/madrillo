#!/usr/bin/env python3
"""Delay a real Linux termination syscall; exit observation must follow it."""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument("monitor", type=Path)
args = parser.parse_args()
monitor = args.monitor.resolve()

# Bounded even if the monitor fails; console EOF also ends the fixture.
workload = """
import select, sys, time
deadline = time.monotonic() + 8
with open(sys.argv[1], 'w', buffering=1) as timestamps:
    while time.monotonic() < deadline:
        timestamps.write(str(time.time_ns() // 1_000_000) + '\\n')
        if select.select([sys.stdin], [], [], 0.01)[0] and not sys.stdin.buffer.read(1):
            break
"""

with tempfile.TemporaryDirectory(prefix="madrillo-exit-time-") as temporary:
    root = Path(temporary)
    state, trace, timestamps = root / "state", root / "trace", root / "timestamps"
    command = ["strace", "-f", "-e", "trace=kill,pidfd_send_signal",
               "-e", "inject=kill,pidfd_send_signal:delay_enter=2s:when=1",
               "-o", str(trace), str(monitor), "--state-dir", str(state),
               "--timeout", "1", "--", sys.executable, "-c", workload, str(timestamps)]
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               start_new_session=True)
    try:
        output, errors = process.communicate(timeout=15)
        assert process.returncode == 0, (output, errors)
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=5)
    calls = trace.read_text()
    assert "(DELAYED)" in calls, ("termination delay was not exercised", calls)
    observed = [int(value) for value in timestamps.read_text().splitlines()]
    assert observed[-1] - observed[0] >= 2000, observed
    retained = json.loads((state / "result.json").read_bytes())
    assert retained["cause"] == "timed_out", retained
    assert retained["leader"] == {"type": "signal", "signal": 9, "core_dumped": False}, retained
    assert retained["cleanup"] == {"type": "complete"}, retained
    assert retained["exited_at_unix_ms"] >= observed[-1], (retained, observed[-1])
    assert not retained["monitor_errors"] and not retained["log_errors"], retained
print("Native exit observation passed: retained timestamp follows delayed termination")
