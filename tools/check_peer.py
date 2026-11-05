#!/usr/bin/env python3
"""Prove Unix control access uses peer identity, including a public socket."""
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import sys
import tempfile
import time


def send(client, message):
    payload = json.dumps({"version": 1, "message": message}).encode()
    client.sendall(struct.pack("!I", len(payload)) + payload)


def receive(client):
    def exact(count):
        result = b""
        while len(result) < count:
            part = client.recv(count - len(result))
            assert part, "control closed unexpectedly"
            result += part
        return result
    size, = struct.unpack("!I", exact(4))
    assert 0 < size <= 1024 * 1024
    result = json.loads(exact(size))
    assert result["version"] == 1
    return result["message"]


monitor, fixture = (str(Path(p).resolve()) for p in sys.argv[1:3])
assert os.name == "posix", "peer qualification requires Unix"
# Explicit /tmp bypasses macOS's per-user private temporary ancestors. Only
# this test directory/socket are made public; retained state stays private.
with tempfile.TemporaryDirectory(prefix="madrillo-peer-", dir="/tmp") as tmp:
    root = Path(tmp)
    root.chmod(0o755)
    endpoint = root / "control.sock"
    child = subprocess.Popen([monitor, "--state-dir", str(root / "state"),
                              "--listen", str(endpoint), "--timeout", "20",
                              "--", fixture, "echo"],
                             stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    try:
        deadline = time.monotonic() + 5
        while not endpoint.exists():
            assert child.poll() is None, child.stderr.read()
            assert time.monotonic() < deadline, "no control socket"
            time.sleep(0.01)
        endpoint.chmod(0o777)
        with socket.socket(socket.AF_UNIX) as owner:
            owner.settimeout(5)
            owner.connect(str(endpoint))
            send(owner, {"type": "attach", "input": True, "streams": []})
            assert receive(owner) == {"type": "attached", "input": True}
            # An actual other account connects through the public path. A
            # connect permission error would not prove peer authentication.
            probe = r'''
import json, os, socket, struct, sys
assert os.geteuid() != int(sys.argv[2]), "probe must use another account"
with socket.socket(socket.AF_UNIX) as client:
    client.settimeout(5)
    client.connect(sys.argv[1])
    data = json.dumps({"version": 1, "message": {"type": "terminate"}}).encode()
    try:
        client.sendall(struct.pack("!I", len(data)) + data)
        assert client.recv(1) == b"", "unauthorized peer received data"
    except (BrokenPipeError, ConnectionResetError):
        pass
print("other account connected and was rejected")
'''
            result = subprocess.run(["sudo", "-n", "-u", "nobody", "/usr/bin/python3",
                                     "-c", probe, str(endpoint), str(os.geteuid())],
                                    capture_output=True, timeout=10, cwd=root)
            assert result.returncode == 0, (result.stdout, result.stderr)
            assert child.poll() is None, "unauthorized termination was honored"
            payload = b"authorized after rejection\n"
            send(owner, {"type": "input", "bytes": list(payload)})
            echoed = bytearray()
            while len(echoed) < len(payload):
                message = receive(owner)
                assert message["type"] == "output", message
                echoed.extend(message["bytes"])
            assert echoed == payload
            send(owner, {"type": "close_input"})
            completed = receive(owner)
            assert completed["type"] == "completed", completed
            assert completed["result"]["leader"] == {"type": "exit", "code": 0}, completed
        assert child.wait(timeout=10) == 0, child.stderr.read()
    finally:
        if child.poll() is None:
            child.kill()
            child.wait(timeout=5)
        child.stderr.close()
print("Unix peer qualification: public socket rejects other UID; owner continues normally")
