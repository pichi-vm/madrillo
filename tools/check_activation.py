#!/usr/bin/env python3
"""Real systemd socket activation, with metadata checked by first workload code."""
import argparse
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument("monitor", type=Path)
parser.add_argument("fixture", type=Path)
args = parser.parse_args()
monitor, fixture = args.monitor.resolve(), args.fixture.resolve()
for terminal in (False, True):
    with tempfile.TemporaryDirectory(prefix="madrillo-activation-") as temporary:
        root = Path(temporary)
        sockets = [root / "one.sock", root / "two.sock"]
        command = ["systemd-socket-activate", "--fdname=first:second"]
        for path in sockets:
            command += ["--listen", str(path)]
        command += [str(monitor), "--state-dir", str(root / "state"), "--timeout", "5"]
        if terminal:
            command.append("--terminal")
        command += ["--", str(fixture), "activation-probe"]
        process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 5
            while not all(path.exists() for path in sockets):
                if process.poll() is not None or time.monotonic() >= deadline:
                    raise AssertionError(("activation did not listen", process.communicate(timeout=1)))
                time.sleep(0.005)
            for path in sockets:
                with socket.socket(socket.AF_UNIX) as peer:
                    peer.settimeout(5)
                    peer.connect(str(path))
                    peer.sendall(b"hello")
                    response = b""
                    while len(response) < 5:
                        packet = peer.recv(5 - len(response))
                        assert packet, "activation peer closed early"
                        response += packet
                    assert response == b"hello", response
            output, errors = process.communicate(timeout=8)
            assert process.returncode == 0, (output, errors)
            result = json.loads((root / "state/result.json").read_bytes())
            assert result["leader"] == {"type": "exit", "code": 37}, result
            assert not result["monitor_errors"], result
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
print("Native systemd activation passed for two named sockets, pipe and terminal launch")
