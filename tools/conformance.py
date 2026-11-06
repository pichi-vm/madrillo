#!/usr/bin/env python3
"""Run and aggregate full conformance. Missing coverage fails before execution.

Qualification CI remains separate. This runner never promotes a smoke run or a
self-declared implementation flag into a full conformance pass.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import platform
import re
import subprocess
import sys
import time
import uuid

ROOT = Path(__file__).resolve().parent.parent
HOSTS = {"Linux": "linux", "Darwin": "macos", "Windows": "windows"}
REQUIRED = {f"C{number:02}" for number in range(1, 31)}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git(*args, directory=None):
    return subprocess.check_output(["git", *args], cwd=directory or ROOT, text=True).strip()


def checkout_identity(directory):
    if Path(git("rev-parse", "--show-toplevel", directory=directory)).resolve() != directory.resolve():
        raise ValueError(f"not an independent source checkout: {directory}")
    # Git normalizes checkout line endings; native builds must use a clean tree.
    if git("status", "--porcelain", "--untracked-files=normal", directory=directory):
        raise ValueError(f"full conformance requires a clean source tree: {directory}")
    submodules = subprocess.check_output(["git", "submodule", "status", "--recursive"],
                                        cwd=directory, text=True)
    if any(line and line[0] in "-+U" for line in submodules.splitlines()):
        raise ValueError(f"submodules must be initialized at their pinned revisions: {directory}")
    return {"revision": git("rev-parse", "HEAD", directory=directory),
            "tree": git("rev-parse", "HEAD^{tree}", directory=directory)}


def identity():
    source = checkout_identity(ROOT)
    components = json.loads((ROOT / "CONFORMANCE.json").read_text()).get("components", {})
    if set(components) != {"pichi"}:
        raise ValueError("full conformance requires pinned Pichi caller sources")
    source["components"] = {}
    for name, pin in components.items():
        if not re.fullmatch(r"[0-9a-f]{40}", pin.get("revision", "")):
            raise ValueError(f"{name}: an immutable source revision is required")
        component = checkout_identity(ROOT / ".qualification/components" / name)
        if component["revision"] != pin["revision"]:
            raise ValueError(f"{name}: checkout does not match the reviewed revision")
        source["components"][name] = component
    return source


def save(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")
    temporary.replace(path)


def toolchain_error(host, compiler):
    if not isinstance(compiler, str):
        return "native compiler evidence is missing"
    target = re.search(r"^host: (\S+)$", compiler, re.MULTILINE)
    if not target:
        return "native compiler target is missing"
    target = target[1]
    if host == "windows" and not target.endswith("-pc-windows-msvc"):
        return "Windows conformance requires the native MSVC toolchain"
    if host == "macos" and not target.endswith("-apple-darwin"):
        return "macOS conformance requires a native Darwin toolchain"
    if host == "linux" and "-linux-" not in target:
        return "Linux conformance requires a native Linux toolchain"
    return None


def coverage(manifest):
    errors = []
    requirements = manifest["requirements"]
    if {item["id"] for item in requirements} != REQUIRED or len(requirements) != 30:
        errors.append("the reviewed C01-C30 requirement set is incomplete or duplicated")
    cases = {case["id"]: case for case in manifest.get("cases", [])}
    if len(cases) != len(manifest.get("cases", [])):
        errors.append("duplicate case IDs")
    for item in requirements:
        hosts = item.get("hosts", [])
        if not hosts or not set(hosts) <= set(HOSTS.values()):
            errors.append(f'{item["id"]}: required native hosts are unspecified')
        if item["status"] != "ready":
            errors.append(f'{item["id"]}: coverage remains {item["status"]}')
        for host in hosts:
            expected = [cases[name] for name in item["tests"] if name in cases and host in cases[name]["hosts"]]
            if not expected or any(name not in cases for name in item["tests"]):
                errors.append(f'{item["id"]}: no complete case mapping for {host}')
    for case in cases.values():
        if not case.get("command") or not case.get("timeout") or not case.get("hosts"):
            errors.append(f'{case["id"]}: incomplete executable case')
        if case.get("kind") == "test" and not case.get("success_markers"):
            errors.append(f'{case["id"]}: no positive execution evidence; exit zero alone is insufficient')
    return errors, cases


def run(args):
    manifest = json.loads((ROOT / "CONFORMANCE.json").read_text())
    host = HOSTS.get(platform.system(), "unsupported")
    report = {"schema": 1, "round": args.round, "invocation": str(uuid.uuid4()),
              "host": host, "os": platform.platform(), "started": datetime.now(timezone.utc).isoformat(),
              "source": None, "cases": {}, "errors": [], "clean": False}
    report["errors"], cases = coverage(manifest)
    try:
        report["source"] = identity()
    except (ValueError, subprocess.SubprocessError) as error:
        report["errors"].append(str(error))
    if host == "unsupported":
        report["errors"].append("unsupported native host")
    if not report["errors"]:
        report["rustc"] = subprocess.check_output(["rustc", "-vV"], cwd=ROOT, text=True)
        if error := toolchain_error(host, report["rustc"]):
            report["errors"].append(error)
    if not report["errors"]:
        for name, case in cases.items():
            if host not in case["hosts"]:
                continue
            log = args.report.parent / (args.report.stem + f"-{name}.log")
            log.parent.mkdir(parents=True, exist_ok=True)
            started = time.monotonic()
            with log.open("wb") as output:
                try:
                    result = subprocess.run(case["command"], cwd=ROOT, stdout=output, stderr=subprocess.STDOUT,
                                            timeout=case["timeout"], check=False)
                    code = result.returncode
                except (OSError, subprocess.TimeoutExpired) as error:
                    output.write(str(error).encode())
                    code = None
            text = log.read_text(encoding="utf-8", errors="replace")
            passed = code == 0 and all(marker in text for marker in case.get("success_markers", []))
            report["cases"][name] = {"passed": passed, "returncode": code, "command": case["command"],
                                     "seconds": time.monotonic() - started, "log": log.name, "sha256": digest(log)}
            if not passed:
                report["errors"].append(f"{name}: failed, timed out, or did not execute required evidence")
        try:
            if identity() != report["source"]:
                report["errors"].append("source changed during execution")
        except (ValueError, subprocess.SubprocessError) as error:
            report["errors"].append(str(error))
    report["clean"] = not report["errors"]
    save(args.report, report)
    print(json.dumps({"clean": report["clean"], "errors": report["errors"]}))
    return 0 if report["clean"] else 1


def aggregate(args):
    manifest = json.loads((ROOT / "CONFORMANCE.json").read_text())
    errors, cases = coverage(manifest)
    reports = []
    try:
        source = identity()
    except (ValueError, subprocess.SubprocessError) as error:
        source = None
        errors.append(str(error))
    for path in args.reports:
        try:
            report = json.loads(path.read_text())
            if (not isinstance(report, dict) or not isinstance(report.get("cases", {}), dict)
                    or not isinstance(report.get("host"), str)
                    or not isinstance(report.get("invocation"), str)
                    or not isinstance(report.get("clean"), bool)):
                raise ValueError("malformed native report")
        except (OSError, ValueError) as error:
            errors.append(f"{path}: {error}")
            report = {}
        reports.append(report)
        if not report.get("clean") or report.get("errors"):
            errors.append(f'{path}: unclean native run')
        if report.get("source") != source:
            errors.append(f'{path}: source identity mismatch')
        if report.get("round") != args.round:
            errors.append(f'{path}: round identity mismatch')
        if error := toolchain_error(report.get("host"), report.get("rustc")):
            errors.append(f"{path}: {error}")
        expected = {name for name, case in cases.items() if report.get("host") in case["hosts"]}
        if set(report.get("cases", {})) != expected:
            errors.append(f'{path}: missing or unexpected native cases')
        for name, outcome in report.get("cases", {}).items():
            if not isinstance(outcome, dict):
                errors.append(f'{path}: {name} is malformed')
                continue
            log_name = outcome.get("log", "")
            if not isinstance(log_name, str):
                errors.append(f'{path}: {name} artifact name is malformed')
                continue
            log = path.parent / log_name
            if Path(log_name).name != log_name or not log.is_file():
                errors.append(f'{path}: {name} artifact absent or invalid')
                continue
            markers = cases.get(name, {}).get("success_markers", [])
            text = log.read_text(encoding="utf-8", errors="replace")
            if (not outcome.get("passed") or outcome.get("returncode") != 0
                    or outcome.get("command") != cases.get(name, {}).get("command")
                    or digest(log) != outcome.get("sha256") or not all(marker in text for marker in markers)):
                errors.append(f'{path}: {name} evidence is invalid')
    if len(reports) != 3 or {report.get("host") for report in reports} != set(HOSTS.values()):
        errors.append("one actual Linux, macOS and Windows report is required")
    ledger = json.loads(args.ledger.read_text()) if args.ledger.exists() else []
    invocations = [report.get("invocation") for report in reports]
    if len(set(invocations)) != 3 or None in invocations:
        errors.append("native invocation IDs are missing or reused")
    if any(entry["round"] == args.round or set(entry["invocations"]) & set(invocations) for entry in ledger):
        errors.append("a round or native invocation cannot be counted twice")
    previous = ledger[-1] if ledger else None
    streak = 0
    if not errors:
        streak = 1 + (previous["clean_streak"] if previous and previous["source"] == source else 0)
    entry = {"round": args.round, "source": source, "invocations": invocations,
             "errors": errors, "clean_streak": streak, "recorded": datetime.now(timezone.utc).isoformat()}
    ledger.append(entry)
    save(args.ledger, ledger)
    print(json.dumps(entry))
    return 0 if not errors else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="action", required=True)
    native = commands.add_parser("run")
    native.add_argument("--round", required=True)
    native.add_argument("--report", type=Path, required=True)
    merge = commands.add_parser("aggregate")
    merge.add_argument("--round", required=True)
    merge.add_argument("--ledger", type=Path, required=True)
    merge.add_argument("reports", type=Path, nargs="*")
    args = parser.parse_args()
    try:
        return run(args) if args.action == "run" else aggregate(args)
    except (ValueError, OSError, KeyError, subprocess.SubprocessError) as error:
        print(f"conformance: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
