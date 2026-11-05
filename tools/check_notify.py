#!/usr/bin/env python3
"""Real datagram filtering plus systemd Type=notify / NotifyAccess=main."""
import json
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import uuid

monitor, fixture = (str(Path(p).resolve()) for p in sys.argv[1:3])
manager = ["--user"] if "--user" in sys.argv[3:] else []

with tempfile.TemporaryDirectory(prefix="madrillo-notify-") as tmp:
    root = Path(tmp)
    target = root / "host.sock"
    with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as receiver:
        receiver.bind(str(target))
        receiver.settimeout(5)
        with subprocess.Popen([monitor, "--state-dir", str(root / "state"), "--sdnotify-socket",
                               str(target), "--timeout", "1", "--", fixture, "notify"],
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE) as child:
            packet = receiver.recv(16385)
            assert packet == b"READY=1\nSTATUS=madrillo relay fixture\nWATCHDOG=1\nERRNO=0", packet
            out, error = child.communicate(timeout=10)
            assert child.returncode == 0, (out, error)
        result = json.loads((root / "state/result.json").read_text())
        assert not result["monitor_errors"], result
        assert not (root / "state/notify.sock").exists(), "relay socket not released"

with tempfile.TemporaryDirectory(prefix="madrillo-notify-service-") as tmp:
    state = Path(tmp) / "state"
    unit = f"madrillo-notify-{uuid.uuid4().hex}.service"
    try:
        subprocess.run(["systemd-run", *manager, "--quiet", "--no-block", "--collect", f"--unit={unit}",
                        "--property=Type=notify", "--property=NotifyAccess=main",
                        "--property=TimeoutStartSec=5", "--property=TimeoutStopSec=5",
                        "--property=RuntimeMaxSec=10", fixture, "exec-notify", monitor, str(state)],
                       check=True, capture_output=True, timeout=10)
        deadline = time.monotonic() + 5
        while True:
            shown = subprocess.run(["systemctl", *manager, "show", unit, "--no-pager",
                                    "--property=ActiveState,StatusText,MainPID"],
                                   capture_output=True, check=True, timeout=5)
            info = dict(line.split("=", 1) for line in shown.stdout.decode().splitlines() if "=" in line)
            if info.get("ActiveState") == "active":
                assert info.get("StatusText") == "madrillo relay fixture", info
                pid = int(info["MainPID"])
                assert pid > 1 and Path(f"/proc/{pid}/comm").read_text().strip() == "madrillo", info
                break
            assert time.monotonic() < deadline, info
            time.sleep(0.05)
    finally:
        subprocess.run(["systemctl", *manager, "stop", unit], capture_output=True, timeout=10)
    result = json.loads((state / "result.json").read_text())
    assert not result["monitor_errors"], result
print("notify qualification: exact safe-field filtering, socket release, real Type=notify readiness with NotifyAccess=main passed")
