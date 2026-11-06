#!/usr/bin/env python3
"""Qualify diagnostic syslog delivery through the real Linux syslog service."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time

monitor, fixture = (str(Path(path).resolve()) for path in sys.argv[1:3])


def records(pid, count):
    deadline = time.monotonic() + 5
    while True:
        query = subprocess.run([
            "journalctl", "--no-pager", "-o", "json", "--since", "-1min",
            "SYSLOG_IDENTIFIER=madrillo", f"_PID={pid}",
        ], capture_output=True, check=True, timeout=10)
        entries = [json.loads(line) for line in query.stdout.splitlines()]
        if len(entries) >= count:
            return entries
        if time.monotonic() >= deadline:
            raise AssertionError(f"missing diagnostic syslog records: {entries!r}; {query.stderr!r}")
        time.sleep(0.05)


with tempfile.TemporaryDirectory(prefix="madrillo-syslog-") as tmp:
    root = Path(tmp)
    for level, minimum, expected_priorities in [("error", 1, {"3"}), ("trace", 4, {"6", "7"})]:
        state = root / level
        command = [monitor, "--state-dir", str(state), "--syslog", "--log-level", level, "--"]
        command += [str(root / "absent-program")] if level == "error" else [fixture, "emit"]
        with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as child:
            try:
                stdout, stderr = child.communicate(timeout=10)
            except BaseException:
                child.kill()
                child.communicate()
                raise
            assert not stdout, stdout
            assert (child.returncode == 0) == (level == "trace"), stderr
            result = json.loads((state / "result.json").read_text())
            assert not any("diagnostics" in error for error in result["monitor_errors"]), result
            entries = records(child.pid, minimum)
        priorities = {entry["PRIORITY"] for entry in entries}
        assert expected_priorities <= priorities, entries
        assert all(entry["_TRANSPORT"] == "syslog" for entry in entries), entries
        assert all(entry["SYSLOG_FACILITY"] == "1" for entry in entries), entries
        if level == "error":
            assert priorities == {"3"}, entries
        else:
            assert result["leader"] == {"type": "exit", "code": 37}, result
            assert any(entry["MESSAGE"].startswith("workload started:") for entry in entries), entries
            assert any(entry["MESSAGE"].startswith("workload completed:") for entry in entries), entries
            assert all(entry["MESSAGE"] != "diagnostic" for entry in entries), entries
            assert b"stderr F diagnostic\n" in (state / "console.log").read_bytes()
print("native syslog: PID attribution, facility, severity, thresholds and workload separation passed")
