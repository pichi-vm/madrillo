#!/usr/bin/env python3
"""Qualify real native VM console lifetime and the direct resize endpoint."""
import os
from pathlib import Path
import platform
import subprocess
import sys


def main():
    monitor, dillo = (Path(value).resolve(strict=True) for value in sys.argv[1:])
    compiler = subprocess.check_output(
        ["rustc", "+nightly-2026-07-06", "-vV"], cwd=dillo, text=True)
    host = next(line.removeprefix("host: ") for line in compiler.splitlines()
                if line.startswith("host: "))
    expected = {"Linux": "-linux-", "Darwin": "-apple-darwin",
                "Windows": "-pc-windows-msvc"}[platform.system()]
    assert expected in host, f"a native compiler is required: {host}"
    print(compiler, flush=True)
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=dillo, text=True)
    print(f"Dillo revision: {revision.strip()}; monitor: {monitor}", flush=True)
    unit = "liveness::tests::unread_pipe_input_does_not_hide_writer_loss"
    result = subprocess.run(
        ["cargo", "+nightly-2026-07-06",
         "test", "--locked", "-p", "dillo-virtio-console", unit,
         "--", "--exact", "--nocapture"], cwd=dillo, check=False,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        encoding="utf-8", errors="replace", timeout=600)
    print(result.stdout, end="", flush=True)
    result.check_returncode()
    assert unit in result.stdout and "1 passed; 0 failed; 0 ignored;" in result.stdout
    assert "console lifetime observed with " in result.stdout
    cases = [
        ("console_lifetime::managed_console_close_stops_a_booted_vm", False,
         ["native VM console lifetime: boot report, ordinary input, cooperative zero exit"]),
        ("console_lifetime::null_console_input_requests_shutdown_without_a_flag", False, []),
        ("console_lifetime::monitor_crash_allows_cooperative_vm_teardown", True,
         ["madrillo crash: unread guest console input stayed backpressured",
          "madrillo crash: confirmed guest input, killed monitor, observed dillo exit zero"]),
        ("console_resize::console_resize_reaches_guest_and_preserves_lifetime", False,
         ["native VM resize: initial size, reconnect, rows/columns, cooperative zero exit"]),
    ]
    for test, ignored, markers in cases:
        command = ["cargo", "+nightly-2026-07-06",
                   "test", "--locked", "-p", "dillo",
                   "--features", "vm-tests", "--test", "boot", test, "--", "--exact",
                   "--test-threads=1", "--nocapture"]
        if ignored:
            command.append("--ignored")
        result = subprocess.run(command, cwd=dillo,
                                env=dict(os.environ, MADRILLO_BIN=str(monitor)),
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                text=True, encoding="utf-8", errors="replace", timeout=600)
        print(result.stdout, end="", flush=True)
        result.check_returncode()
        assert test in result.stdout, f"missing native test: {test}"
        assert "1 passed; 0 failed; 0 ignored;" in result.stdout, "VM test did not execute"
        for marker in markers:
            assert marker in result.stdout, f"missing VM evidence: {marker}"
    print("native Dillo console lifetime and resize: four real VM cases passed", flush=True)


if __name__ == "__main__":
    main()
