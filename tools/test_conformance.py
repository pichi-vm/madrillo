"""Synthetic negative tests for the evidence gate; these are not conformance runs."""
import argparse
import contextlib
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("conformance", Path(__file__).with_name("conformance.py"))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class EvidenceGate(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source = {"revision": "test-revision", "tree": "test-tree"}
        self.manifest = {"requirements": [
            {"id": f"C{index:02}", "status": "ready", "hosts": list(gate.HOSTS.values()), "tests": ["fixture"]}
            for index in range(1, 31)], "cases": [
                {"id": "fixture", "hosts": list(gate.HOSTS.values()), "kind": "test", "command": ["synthetic-fixture"],
                 "timeout": 1, "success_markers": ["fixture executed"]}]}
        (self.root / "CONFORMANCE.json").write_text(json.dumps(self.manifest))
        self.root_patch = patch.object(gate, "ROOT", self.root)
        self.root_patch.start()
        self.addCleanup(self.root_patch.stop)
        self.identity_patch = patch.object(gate, "identity", side_effect=lambda: self.source)
        self.identity_patch.start()
        self.addCleanup(self.identity_patch.stop)

    def reports(self, round_id):
        paths = []
        targets = {"linux": "x86_64-unknown-linux-gnu", "macos": "aarch64-apple-darwin",
                   "windows": "x86_64-pc-windows-msvc"}
        for host in gate.HOSTS.values():
            path = self.root / f"{round_id}-{host}.json"
            log = path.with_suffix(".log")
            log.write_bytes(b"fixture executed\n")
            path.write_text(json.dumps({
                "source": self.source, "round": round_id, "host": host, "invocation": f"{round_id}-{host}",
                "rustc": f"rustc test-fixture\nhost: {targets[host]}\n",
                "clean": True, "errors": [], "cases": {"fixture": {"passed": True, "returncode": 0,
                "command": ["synthetic-fixture"], "log": log.name, "sha256": hashlib.sha256(log.read_bytes()).hexdigest()}}}))
            paths.append(path)
        return paths

    def aggregate(self, round_id, paths):
        ledger = self.root / "ledger.json"
        with contextlib.redirect_stdout(io.StringIO()):
            code = gate.aggregate(argparse.Namespace(round=round_id, reports=paths, ledger=ledger))
        return code, json.loads(ledger.read_text())[-1]["clean_streak"]

    def test_three_fresh_rounds_count_and_reuse_resets(self):
        for index in range(1, 4):
            name = f"round-{index}"
            self.assertEqual(self.aggregate(name, self.reports(name)), (0, index))
        self.assertEqual(self.aggregate("round-3", self.reports("round-3")), (1, 0))

    def test_missing_native_report_resets_then_starts_at_one(self):
        self.assertEqual(self.aggregate("first", self.reports("first")), (0, 1))
        paths = self.reports("missing")
        paths[-1].unlink()
        self.assertEqual(self.aggregate("missing", paths), (1, 0))
        self.assertEqual(self.aggregate("next", self.reports("next")), (0, 1))

    def test_changed_source_starts_a_new_streak(self):
        self.assertEqual(self.aggregate("first", self.reports("first")), (0, 1))
        self.source = {"revision": "changed", "tree": "changed"}
        self.assertEqual(self.aggregate("next", self.reports("next")), (0, 1))

    def test_changed_caller_source_starts_a_new_streak(self):
        self.source["components"] = {"pichi": {"revision": "before", "tree": "before"}}
        self.assertEqual(self.aggregate("caller-before", self.reports("caller-before")), (0, 1))
        self.source["components"]["pichi"] = {"revision": "after", "tree": "after"}
        self.assertEqual(self.aggregate("caller-after", self.reports("caller-after")), (0, 1))

    def test_failed_evidence_cannot_be_overridden_by_clean_flag(self):
        for kind in ["hash", "marker", "missing", "status", "invalid-json"]:
            with self.subTest(kind=kind):
                paths = self.reports(kind)
                report = json.loads(paths[0].read_text())
                case = report["cases"]["fixture"]
                log = paths[0].parent / case["log"]
                if kind == "hash": log.write_bytes(b"modified")
                if kind == "marker":
                    log.write_bytes(b"zero tests ran")
                    case["sha256"] = gate.digest(log)
                if kind == "missing": log.unlink()
                if kind == "status": case["returncode"] = 1
                paths[0].write_text(json.dumps(report) if kind != "invalid-json" else "{")
                self.assertEqual(self.aggregate(kind, paths), (1, 0))

    def test_pending_requirement_cannot_pass_with_successful_suites(self):
        self.manifest["requirements"][6]["status"] = "pending"
        (self.root / "CONFORMANCE.json").write_text(json.dumps(self.manifest))
        self.assertEqual(self.aggregate("pending", self.reports("pending")), (1, 0))

    def test_non_native_or_missing_compiler_evidence_resets(self):
        self.assertEqual(self.aggregate("native", self.reports("native")), (0, 1))
        for host, target in [("windows", "x86_64-pc-windows-gnu"),
                             ("macos", "x86_64-unknown-linux-gnu"),
                             ("linux", "aarch64-apple-darwin"), ("windows", None)]:
            name = f"{host}-{target}"
            paths = self.reports(name)
            path = next(path for path in paths if path.name.endswith(f"-{host}.json"))
            report = json.loads(path.read_text())
            if target is None:
                report.pop("rustc")
            else:
                report["rustc"] = f"host: {target}\n"
            path.write_text(json.dumps(report))
            self.assertEqual(self.aggregate(name, paths), (1, 0))

    def test_windows_gnu_is_rejected_before_running_cases(self):
        report = self.root / "gnu.json"
        with patch.object(gate.platform, "system", return_value="Windows"), \
             patch.object(gate.subprocess, "check_output", return_value="host: x86_64-pc-windows-gnu\n"), \
             patch.object(gate.subprocess, "run") as execute, contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(gate.run(argparse.Namespace(round="gnu", report=report)), 1)
            execute.assert_not_called()
        result = json.loads(report.read_text())
        self.assertFalse(result["clean"])
        self.assertEqual(result["cases"], {})
        self.assertIn("Windows conformance requires the native MSVC toolchain", result["errors"])


class NativeSourceIdentity(unittest.TestCase):
    def test_dirty_changed_or_missing_component_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)

            def commit(directory):
                subprocess.run(["git", "add", "."], cwd=directory, check=True, capture_output=True)
                subprocess.run(["git", "-c", "user.name=Conformance Test", "-c",
                                "user.email=conformance@example.invalid", "commit", "-m", "fixture"],
                               cwd=directory, check=True, capture_output=True)
                return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=directory, text=True).strip()

            subprocess.run(["git", "init", "--quiet", str(root)], check=True)
            (root / ".gitignore").write_text(".qualification/\n")
            pins = {}
            for name in ("pichi", "dillo"):
                directory = root / ".qualification/components" / name
                directory.mkdir(parents=True)
                subprocess.run(["git", "init", "--quiet", str(directory)], check=True)
                (directory / "source").write_text("original\n")
                pins[name] = {"revision": commit(directory)}
            (root / "CONFORMANCE.json").write_text(json.dumps({"components": pins}))
            commit(root)
            with patch.object(gate, "ROOT", root):
                self.assertEqual(gate.identity()["components"]["pichi"]["revision"], pins["pichi"]["revision"])
                caller = root / ".qualification/components/pichi"
                (caller / "source").write_text("changed\n")
                with self.assertRaisesRegex(ValueError, "clean source tree"):
                    gate.identity()
                commit(caller)
                with self.assertRaisesRegex(ValueError, "reviewed revision"):
                    gate.identity()
                # An empty child directory must not fall back to its parent's
                # Git identity and masquerade as the component checkout.
                empty = root / ".qualification/empty"
                empty.mkdir()
                with self.assertRaisesRegex(ValueError, "independent source checkout"):
                    gate.checkout_identity(empty)


if __name__ == "__main__":
    unittest.main()
