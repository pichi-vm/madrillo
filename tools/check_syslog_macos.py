#!/usr/bin/env python3
"""Require delivery through macOS syslogd, beyond successful socket writes."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time

if sys.platform != "darwin":
    raise SystemExit("this qualification requires native macOS")

monitor, fixture = (str(Path(path).resolve()) for path in sys.argv[1:3])


def delivered(pid, text, since):
    # Apple's default asl.conf stores error-level messages in system.log.
    # Keep this read bounded and filter by this exact process, including when
    # syslogd rotates the file between the initial observation and delivery.
    deadline = time.monotonic() + 10
    identity = f"madrillo[{pid}]".encode()
    while True:
        with Path("/var/log/system.log").open("rb") as log:
            log.seek(0, 2)
            log.seek(max(0, log.tell() - 1024 * 1024))
            lines = [line for line in log.read().splitlines() if identity in line]
        if any(text.encode() in line for line in lines):
            print(f"macOS syslogd delivered: {lines!r}")
            return lines
        if time.monotonic() >= deadline:
            query = subprocess.run([
                "/usr/bin/syslog", "-F", "raw", "-k", "Sender", "madrillo",
                "-k", "PID", str(pid), "-k", "Time", "ge", str(since),
            ], capture_output=True, timeout=10)
            unified = subprocess.run([
                "/usr/bin/log", "show", "--last", "1m", "--style", "ndjson",
                "--info", "--debug", "--predicate", f"processID == {pid}",
            ], capture_output=True, timeout=15)
            raise AssertionError(
                f"missing macOS syslogd delivery: {lines!r}; "
                f"ASL query status {query.returncode}: "
                f"{query.stdout[-8192:]!r}; {query.stderr[-8192:]!r}; "
                f"unified log status {unified.returncode}: "
                f"{unified.stdout[-8192:]!r}; {unified.stderr[-8192:]!r}"
            )
        time.sleep(0.1)


with tempfile.TemporaryDirectory(prefix="madrillo-syslog-") as tmp:
    root = Path(tmp)
    for level in ["error", "trace"]:
        state = root / level
        since = int(time.time())
        # A failed exit hook produces an error-level monitor diagnostic after
        # successful capture. The workload's own exit and stderr stay separate.
        command = [
            monitor, "--state-dir", str(state), "--syslog", "--log-level", level,
            "--exit-command", fixture, "--exit-command-arg", "exit",
            "--exit-command-arg", "73", "--", fixture, "emit",
        ]
        with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as child:
            try:
                stdout, stderr = child.communicate(timeout=15)
            except BaseException:
                child.kill()
                child.communicate()
                raise
            assert not stdout, stdout
            assert child.returncode != 0, stderr
            assert b"exit command failed:" in stderr, stderr
            assert b"diagnostics syslog:" not in stderr, stderr
            result = json.loads((state / "result.json").read_text())
            assert result["leader"] == {"type": "exit", "code": 37}, result
            assert not any("diagnostics" in error for error in result["monitor_errors"]), result
            entries = delivered(child.pid, "exit command failed:", since)
            assert all(b": diagnostic" not in entry for entry in entries), entries
            workload_log = (state / "console.log").read_bytes()
            assert b"stderr F diagnostic\n" in workload_log, workload_log
            assert b"exit command failed:" not in workload_log, workload_log
print("native macOS syslogd: fatal delivery, process identity and workload separation passed")
