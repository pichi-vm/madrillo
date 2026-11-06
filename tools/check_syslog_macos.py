#!/usr/bin/env python3
"""Require native macOS diagnostic delivery with process attribution."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time

if sys.platform != "darwin":
    raise SystemExit("this qualification requires native macOS")

monitor, fixture = (str(Path(path).resolve()) for path in sys.argv[1:3])


def failure_context(state, stderr):
    print(f"monitor stderr: {stderr!r}", flush=True)
    for name in ["result.json", "console.log"]:
        path = state / name
        if path.exists():
            print(f"{name}: {path.read_bytes()[-8192:]!r}", flush=True)
    query = subprocess.run([
        "/usr/bin/log", "show", "--last", "1m", "--style", "ndjson",
        "--info", "--debug", "--predicate", 'eventMessage CONTAINS[c] "madrillo"',
    ], capture_output=True, timeout=15)
    print(f"native failure context ({query.returncode}):", flush=True)
    for line in query.stdout.splitlines():
        try:
            record = json.loads(line)
        except ValueError:
            continue
        if "eventMessage" in record:
            print(record.get("timestamp"), record["eventMessage"], flush=True)
    print(f"query stderr: {query.stderr[-8192:]!r}", flush=True)
    deadline = time.monotonic() + 5
    while True:
        reports = list((Path.home() / "Library/Logs/DiagnosticReports").glob("madrillo*.ips"))
        reports += list(Path("/Library/Logs/DiagnosticReports").glob("madrillo*.ips"))
        if reports or time.monotonic() >= deadline:
            break
        time.sleep(0.1)
    for path in sorted(reports, key=lambda path: path.stat().st_mtime)[-3:]:
        text = path.read_text()
        try:
            # Apple's .ips format has a one-line JSON header followed by the
            # complete JSON report. Retain the cause and faulting thread only.
            header, report = text.split("\n", 1)
            report = json.loads(report)
            fields = {key: report.get(key) for key in [
                "pid", "procName", "procPath", "exception", "termination", "asi",
            ]}
            threads = report.get("threads", [])
            fault = report.get("faultingThread")
            if isinstance(fault, int) and 0 <= fault < len(threads):
                fields["thread"] = threads[fault]
            print(f"crash {path.name}: {header}\n{json.dumps(fields)}", flush=True)
        except (ValueError, TypeError):
            print(f"crash {path.name}: {text[:16384]}", flush=True)


def delivered(pid, text):
    # Query Apple's actual service; a successful logging API call alone is not
    # evidence of delivery. Filter narrowly, and permit asynchronous ingestion.
    deadline = time.monotonic() + 20
    predicate = f'processID == {pid} AND subsystem == "org.pichi.madrillo"'
    while True:
        query = subprocess.run([
            "/usr/bin/log", "show", "--last", "1m", "--style", "ndjson",
            "--info", "--debug", "--predicate", predicate,
        ], capture_output=True, timeout=15, check=True)
        entries = [json.loads(line) for line in query.stdout.splitlines() if line]
        entries = [entry for entry in entries if "eventMessage" in entry]
        matches = [entry for entry in entries if text in entry["eventMessage"]]
        if matches:
            for entry in matches:
                assert entry["processID"] == pid, entry
                assert Path(entry["processImagePath"]) == Path(monitor), entry
                assert entry["subsystem"] == "org.pichi.madrillo", entry
                assert entry["category"] == "monitor", entry
                assert entry["messageType"] == "Error", entry
            print(f"macOS unified logging delivered: {matches!r}")
            return entries
        if time.monotonic() >= deadline:
            raise AssertionError(
                f"missing macOS diagnostic delivery: {entries!r}; "
                f"{query.stderr[-8192:]!r}"
            )
        time.sleep(0.25)


with tempfile.TemporaryDirectory(prefix="madrillo-syslog-") as tmp:
    root = Path(tmp)
    for level in ["error", "trace"]:
        state = root / level
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
            if result["leader"] != {"type": "exit", "code": 37}:
                failure_context(state, stderr)
            assert result["leader"] == {"type": "exit", "code": 37}, result
            assert not any("diagnostics" in error for error in result["monitor_errors"]), result
            entries = delivered(child.pid, "exit command failed:")
            assert all(
                entry["eventMessage"].rsplit(": ", 1)[-1] != "diagnostic"
                for entry in entries
            ), entries
            if level == "error":
                assert all(entry["messageType"] == "Error" for entry in entries), entries
            workload_log = (state / "console.log").read_bytes()
            assert b"stderr F diagnostic\n" in workload_log, workload_log
            assert b"exit command failed:" not in workload_log, workload_log
print("native macOS unified logging: fatal delivery, process identity and workload separation passed")
