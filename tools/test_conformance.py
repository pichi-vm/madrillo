"""Synthetic negative tests for the evidence gate; these are not conformance runs."""
import argparse
import contextlib
import hashlib
import importlib.util
import io
import json
from pathlib import Path
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
        for host in gate.HOSTS.values():
            path = self.root / f"{round_id}-{host}.json"
            log = path.with_suffix(".log")
            log.write_bytes(b"fixture executed\n")
            path.write_text(json.dumps({
                "source": self.source, "round": round_id, "host": host, "invocation": f"{round_id}-{host}",
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


if __name__ == "__main__":
    unittest.main()
