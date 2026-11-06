#!/usr/bin/env python3
"""Qualify the native caller and reconciliation; real VM/build cases are separate."""
import os
from pathlib import Path
import platform
import subprocess
import sys


def run(command, pichi, environment, markers):
    result = subprocess.run(command, cwd=pichi, env=environment,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            text=True, encoding="utf-8", errors="replace", timeout=1500)
    print(result.stdout, end="", flush=True)
    result.check_returncode()
    for marker in markers:
        assert marker in result.stdout, f"missing native caller evidence: {marker}"


def main():
    monitor, pichi = (Path(value).resolve(strict=True) for value in sys.argv[1:])
    compiler = subprocess.check_output(
        ["rustc", "+nightly-2026-07-06", "-vV"], cwd=pichi, text=True)
    host = next(line.removeprefix("host: ") for line in compiler.splitlines()
                if line.startswith("host: "))
    expected = {"Linux": "-linux-", "Darwin": "-apple-darwin",
                "Windows": "-pc-windows-msvc"}[platform.system()]
    assert expected in host, f"a native compiler is required: {host}"
    assert monitor.with_name("madrillo-fixture" + monitor.suffix).is_file(), "native fixture missing"
    print(compiler, flush=True)
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=pichi, text=True)
    print(f"Pichi revision: {revision.strip()}; monitor: {monitor}", flush=True)
    environment = dict(os.environ, PICHI_MADRILLO=str(monitor), MADRILLO_BIN=str(monitor))
    markers = [
        "test cmd::workload::tests::stop_without_monitor_or_result_preserves_unknown_state_and_pins ... ok",
        "test cmd::workload::record::tests::storage_cleanup_requires_complete_evidence_and_is_idempotent ... ok",
        "test cmd::workload::record::tests::missing_records_are_read_only_and_partial_publication_fails_closed ... ok",
        "test cmd::workload::record::tests::image_gc_preserves_pins_and_guest_selected_build_inputs ... ok",
        "test detached_storage_stays_reserved_until_workload_exit ... ok",
        "test cmd::workload::resize::tests::resize_reconnects_and_delivers_before_client_close ... ok",
    ]
    native = ["native_monitor_launch_and_async_control",
              "monitor_crash_preserves_unknown_state_and_cleanup_inputs"]
    crash = "native monitor crash: unknown state and cleanup inputs retained"
    if os.name == "nt":
        script = Path(__file__).with_name("check_windows_caller.ps1").resolve()
        run(["pwsh", "-NoLogo", "-NoProfile", "-NonInteractive", "-File", str(script),
             "-PichiDirectory", str(pichi)], pichi, environment, markers + [
                *(f"cmd::workload::tests::{name}" for name in native),
                "1 passed; 0 failed; 0 ignored;",
                crash, "native Windows caller loss passed (console=false)",
                "native Windows caller loss passed (console=true)",
                "native Windows CLI terminal: run, input, resize, detach, reattach, detached start and stop passed"])
    else:
        cargo = ["cargo", "+nightly-2026-07-06", "test", "--locked", "-p", "pichi"]
        run([*cargo, "--lib", "--test", "cmd_workload", "--test", "cmd_build",
             "--test", "cmd_run", "--test", "cmd_rmi"], pichi, environment, markers + [
                "test caller_group_loss_preserves_workload_and_reconnection ... ok",
                "test terminal_restoration_and_hangup_preserve_workload ... ok",
                "test detached_exit_removes_the_child_resize_socket ... ok",
                "test detached_launch_is_discoverable_and_later_wait_reads_retained_exit ... ok",
                "test build_port_remains_direct_and_guest_first_through_monitor_launch ... ok"])
        for name in native:
            test = "cmd::workload::tests::" + name
            run([*cargo, "--lib", test, "--", "--exact", "--ignored", "--nocapture"],
                pichi, environment, [test, "1 passed; 0 failed; 0 ignored;"] +
                ([crash] if name.startswith("monitor_crash") else []))
    print("native Pichi caller and crash reconciliation passed", flush=True)


if __name__ == "__main__":
    main()
