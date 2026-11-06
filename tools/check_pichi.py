#!/usr/bin/env python3
"""Qualify the native caller and reconciliation; real VM/build cases are separate."""
import json
import os
from pathlib import Path
import platform
import subprocess
import sys


def run(command, pichi, environment, markers, timeout=1500):
    result = subprocess.run(command, cwd=pichi, env=environment,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            text=True, encoding="utf-8", errors="replace", timeout=timeout)
    print(result.stdout, end="", flush=True)
    result.check_returncode()
    for marker in markers:
        assert marker in result.stdout, f"missing native execution evidence: {marker}"


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
        "test vm_timeout_cleanup_stops_workloads_after_client_exit ... ok",
        "test cmd::workload::resize::tests::resize_reconnects_after_receiver_consumes_each_update ... ok",
        "test cmd::device_buses::tests::both_pmi_architectures_supply_declared_capacity ... ok",
        "test cmd::device_buses::tests::preferences_fall_back_without_creating_slots ... ok",
        "test cmd::device_buses::tests::build_placement_preserves_named_port_handles_and_mounts ... ok",
        "test cmd::device_buses::tests::truncated_or_missing_metadata_is_rejected ... ok",
    ]
    native = ["native_monitor_launch_and_async_control",
              "monitor_crash_preserves_unknown_state_and_cleanup_inputs"]
    crash = "native monitor crash: unknown state and cleanup inputs retained"
    cargo = ["cargo", "+nightly-2026-07-06", "test", "--locked", "-p", "pichi"]
    lifetime = ["cargo", "+1.94.0", "test", "--locked", "--all-features", "--test", "control"]
    if os.name == "nt":
        script = pichi / ".github/scripts/windows-test-runner.ps1"
        assert script.is_file(), "native Windows test runner is missing"
        # Cargo splits a string runner on whitespace, without shell unquoting.
        # Its TOML array keeps native paths (including spaces) as one argument.
        runner = ["pwsh", "-NoLogo", "-NoProfile", "-NonInteractive", "-File", str(script)]
        cargo += ["--config", f"target.{host}.runner={json.dumps(runner)}"]
        lifetime += ["--config", f"target.{host}.runner={json.dumps(runner)}"]
    else:
        markers += [
            "test caller_group_loss_preserves_workload_and_reconnection ... ok",
            "test terminal_restoration_and_hangup_preserve_workload ... ok",
            "test detached_exit_removes_the_child_resize_socket ... ok",
            "test detached_launch_is_discoverable_and_later_wait_reads_retained_exit ... ok",
            "test build_port_remains_direct_and_guest_first_through_monitor_launch ... ok"]
    test = "caller_lifetime::caller_loss_preserves_pipe_and_terminal"
    lifetime += [test, "--", "--exact", "--nocapture"]
    if os.name == "nt":
        lifetime.append("--ignored")
    run(lifetime, Path(__file__).resolve().parents[1], environment,
        [f"test {test} ... ok", "1 passed; 0 failed; 0 ignored;"] + [
            f"caller lifetime: terminal={terminal}, attached={attached}, loss={loss} passed"
            for terminal in ("false", "true") for attached in ("false", "true")
            for loss in ("exit", "crash", "hangup")])
    run([*cargo, "--lib", "--test", "cmd_workload", "--test", "cmd_build",
         "--test", "cmd_run", "--test", "cmd_rmi"], pichi, environment, markers)
    for name in native:
        test = "cmd::workload::tests::" + name
        evidence = [crash] if name.startswith("monitor_crash") else []
        if os.name == "nt" and name == "native_monitor_launch_and_async_control":
            evidence.append("native Pichi exact inheritance: opaque bytes, excluded handle and EOF passed")
        run([*cargo, "--lib", test, "--", "--exact", "--ignored", "--nocapture"],
            pichi, environment, [test, "1 passed; 0 failed; 0 ignored;", *evidence])
    if os.name == "nt":
        test = "native_placement_precedes_workload_and_helpers"
        run(["cargo", "+1.94.0", "test", "--locked", "--all-features", "--test", "core",
             "--config", f"target.{host}.runner={json.dumps(runner)}", test,
             "--", "--exact", "--ignored", "--nocapture"],
            Path(__file__).resolve().parents[1], environment,
            [f"test {test} ... ok", "1 passed; 0 failed; 0 ignored;"] + [
                f"native placement: terminal={terminal}, early leader/helper membership and outside exit helper passed"
                for terminal in ("false", "true")])
        for target, test, evidence in [
            (["--lib"], "cmd::workload::tests::job::caller_loss_preserves_workload",
             ["native Windows caller loss passed (console=false)",
              "native Windows caller loss passed (console=true)"]),
            (["--test", "cmd_run"], "windows_terminal::interactive_resize_input_and_reattach",
             ["native Windows CLI terminal: run, input, resize, detach, reattach, detached start and stop passed"]),
        ]:
            run([*cargo, *target, test, "--", "--exact", "--ignored", "--nocapture"],
                pichi, environment, [test, "1 passed; 0 failed; 0 ignored;", *evidence])
    print("native Pichi caller and crash reconciliation passed", flush=True)


if __name__ == "__main__":
    main()
