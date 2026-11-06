#!/usr/bin/env python3
"""Qualify real native VM console lifetime against the selected monitor."""
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
    cases = [
        ("managed_console_close_stops_a_booted_vm", False,
         ["native VM console lifetime: boot report, ordinary input, cooperative zero exit"]),
        ("null_console_input_requests_shutdown_without_a_flag", False, []),
        ("monitor_crash_allows_cooperative_vm_teardown", True,
         ["madrillo crash: unread guest console input stayed backpressured",
          "madrillo crash: confirmed guest input, killed monitor, observed dillo exit zero"]),
    ]
    for name, ignored, markers in cases:
        test = "console_lifetime::" + name
        command = ["cargo", "+nightly-2026-07-06", "test", "--locked", "-p", "dillo",
                   "--features", "vm-tests", "--test", "boot", test, "--", "--exact",
                   "--test-threads=1", "--nocapture"]
        if ignored:
            command.append("--ignored")
        result = subprocess.run(command, cwd=dillo,
                                env=dict(os.environ, MADRILLO_BIN=str(monitor)),
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                text=True, encoding="utf-8", errors="replace")
        print(result.stdout, end="", flush=True)
        result.check_returncode()
        assert test in result.stdout, f"missing native test: {test}"
        assert "1 passed; 0 failed; 0 ignored;" in result.stdout, "VM test did not execute"
        for marker in markers:
            assert marker in result.stdout, f"missing VM evidence: {marker}"
    print("native Dillo console lifetime: three real VM cases passed", flush=True)


if __name__ == "__main__":
    main()
