#!/usr/bin/env python3
"""Observe real monitor/workload OOM adjustments without exhausting memory."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument("monitor", type=Path)
parser.add_argument("fixture", type=Path)
parser.add_argument("--expect", choices=("protected", "denied"), required=True)
args = parser.parse_args()
monitor, fixture = args.monitor.resolve(), args.fixture.resolve()
for terminal in (False, True):
    with tempfile.TemporaryDirectory(prefix="madrillo-oom-adjust-") as temporary:
        root = Path(temporary)
        state, report = root / "state", root / "scores.json"
        command = [str(fixture), "exec-with-oom-score", "200", str(monitor),
                   "--state-dir", str(state)]
        command += ["--exit-command", str(fixture)]
        for argument in ["exit-hook", str(state), str(root / "hook-marker"), "0"]:
            command += ["--exit-command-arg", argument]
        if terminal:
            command.append("--terminal")
        command += ["--", str(fixture), "oom-probe", str(report)]
        completed = subprocess.run(command, capture_output=True, timeout=10, check=True)
        scores = json.loads(report.read_bytes())
        result = json.loads((state / "result.json").read_bytes())
        warnings = "\n".join(result["monitor_warnings"])
        assert scores["workload"] == 200, scores
        assert int((state / "hook-oom-score").read_text()) == 200
        if args.expect == "protected":
            assert scores["monitor"] == -1000, (scores, completed.stderr)
            assert "OOM protection unavailable" not in warnings, result
        else:
            assert scores["monitor"] == 200, scores
            assert "OOM protection unavailable" in warnings, result
        assert result["leader"] == {"type": "exit", "code": 37}, result
        assert not result["monitor_errors"], result
print(f"Native pipe/terminal OOM protection {args.expect}: workload and exit-helper restoration passed")
