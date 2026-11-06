#!/usr/bin/env python3
"""Run Pichi's real build/boot acceptance on the host's native hypervisor."""
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile

from check_pichi import run


def main():
    monitor, pichi = (Path(value).resolve(strict=True) for value in sys.argv[1:])
    compiler = subprocess.check_output(
        ["rustc", "+nightly-2026-07-06", "-vV"], cwd=pichi, text=True)
    host = next(line.removeprefix("host: ") for line in compiler.splitlines()
                if line.startswith("host: "))
    expected = {"Linux": "-linux-", "Darwin": "-apple-darwin",
                "Windows": "-pc-windows-msvc"}[platform.system()]
    assert expected in host, f"a native compiler is required: {host}"
    architecture = host.split("-", 1)[0]
    assert architecture in ("x86_64", "aarch64"), f"unsupported guest architecture: {host}"
    guest = architecture + "-unknown-linux-musl"
    print(compiler, flush=True)
    subprocess.run(["git", "rev-parse", "HEAD"], cwd=pichi, check=True)
    print(f"Native monitor: {monitor}; guest tools: {guest}", flush=True)
    environment = dict(os.environ, PICHI_MADRILLO=str(monitor), MADRILLO_BIN=str(monitor),
                       CARGO_NET_GIT_FETCH_WITH_CLI="true")
    environment[f"CARGO_TARGET_{guest.upper().replace('-', '_')}_LINKER"] = "rust-lld"
    if platform.system() != "Linux":
        filesystem = Path(environment.get("PICHI_TEST_ROOT_FILESYSTEM", ""))
        assert filesystem.is_file(), "set PICHI_TEST_ROOT_FILESYSTEM to Pichi's portable ext4 fixture"
        environment["PICHI_TEST_ROOT_FILESYSTEM"] = str(filesystem.resolve())
    if not environment.get("PICHI_TEST_STRIP"):
        sysroot = subprocess.check_output(
            ["rustc", "+nightly-2026-07-06", "--print", "sysroot"], text=True).strip()
        strip = Path(sysroot) / "lib/rustlib" / host / "bin" / (
            "llvm-objcopy.exe" if os.name == "nt" else "llvm-objcopy")
        assert strip.is_file(), "install nightly-2026-07-06's llvm-tools component"
        environment["PICHI_TEST_STRIP"] = str(strip)
    cargo = ["cargo", "+nightly-2026-07-06"]
    if os.name == "nt":
        runner = pichi / ".github/scripts/windows-test-runner.ps1"
        assert runner.is_file(), "native Windows test runner is missing"
        arguments = ["pwsh", "-NoLogo", "-NoProfile", "-NonInteractive", "-File", str(runner)]
        cargo += ["--config", f"target.{host}.runner={json.dumps(arguments)}"]
    # These are the same pinned static fixtures as Pichi's native VM workflow.
    # Install outside the source tree; only ordinary Cargo build caches persist.
    with tempfile.TemporaryDirectory(prefix="madrillo-vm-tools-") as directory:
        for name, revision in [
            ("arma", "3297920b46122689fb13ca41acd61cbe94ac1de7"),
            ("corium", "c33ae0e01d58b657f8f62cb91672885326a903b9"),
        ]:
            print(f"Building pinned guest fixture: {name}@{revision}", flush=True)
            subprocess.run(
                [*cargo, "install", "-Z", "bindeps", "--locked", "--git",
                 f"https://github.com/pichi-vm/{name}", "--rev", revision,
                 "--target", guest, "--root", directory, "--target-dir",
                 str(pichi / "target/qualification/guest-tools"), name],
                cwd=pichi, env=environment, check=True, timeout=1800)
        environment["PICHI_TEST_GUEST_ARMA"] = str(Path(directory) / "bin/arma")
        environment["PICHI_TEST_REPLACEMENT_CORIUM"] = str(Path(directory) / "bin/corium")
        for target, test in [
            ("build_boot", "pichi_build_boots_official_image_to_poweroff"),
            ("build_submodules", "pichi_build_checks_committed_submodule_stores"),
            ("staged_images", "staged_images_create_extend_and_compose"),
        ]:
            print(f"Running native VM suite: {test}", flush=True)
            run([*cargo, "test", "--locked", "-p", "pichi", "--features", "vm-tests",
                 "--test", target, test, "--", "--exact", "--test-threads=1", "--nocapture"],
                pichi, environment, [test, "1 passed; 0 failed; 0 ignored;"], timeout=3600)
            print(f"native VM suite passed: {test}", flush=True)
    print("native Pichi VM/build: three real build-and-boot suites passed", flush=True)


if __name__ == "__main__":
    main()
