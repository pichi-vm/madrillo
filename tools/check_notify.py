#!/usr/bin/env python3
"""Native notification delivery/failures and real systemd service attribution."""
import contextlib
import json
from pathlib import Path
import socket
import struct
import subprocess
import sys
import tempfile
import time
import uuid

monitor, fixture = (str(Path(p).resolve()) for p in sys.argv[1:3])
manager = ["--user"] if "--user" in sys.argv[3:] else []


@contextlib.contextmanager
def running(arguments, **options):
    child = subprocess.Popen(arguments, stdout=subprocess.PIPE, stderr=subprocess.PIPE, **options)
    try:
        yield child
    finally:
        if child.poll() is None:
            # Let the monitor forward termination and reap its workload first.
            child.terminate()
            try:
                child.wait(timeout=8)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=5)
        child.stdout.close()
        child.stderr.close()


def message(connection):
    def exact(size):
        data = bytearray()
        while len(data) < size:
            part = connection.recv(size - len(data))
            assert part, "control closed before completion"
            data.extend(part)
        return data

    length = struct.unpack(">I", exact(4))[0]
    assert length <= 1024 * 1024, length
    return json.loads(exact(length))["message"]

with tempfile.TemporaryDirectory(prefix="madrillo-notify-") as tmp:
    root = Path(tmp)
    target = root / "host.sock"
    with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as receiver:
        receiver.bind(str(target))
        receiver.settimeout(5)
        with running([monitor, "--state-dir", str(root / "state"), "--sdnotify-socket",
                      str(target), "--timeout", "1", "--", fixture, "notify"]) as child:
            packet = receiver.recv(16385)
            assert packet == b"READY=1\nSTATUS=madrillo relay fixture\nWATCHDOG=1\nERRNO=0", packet
            out, error = child.communicate(timeout=10)
            assert child.returncode == 0, (out, error)
        result = json.loads((root / "state/result.json").read_text())
        assert not result["monitor_errors"], result
        assert not (root / "state/notify.sock").exists(), "relay socket not released"

with tempfile.TemporaryDirectory(prefix="madrillo-notify-large-") as tmp:
    root = Path(tmp)
    target = root / "host.sock"
    with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as receiver:
        receiver.bind(str(target))
        receiver.setblocking(False)
        with running([monitor, "--state-dir", str(root / "state"), "--sdnotify-socket",
                      str(target), "--timeout", "1", "--", fixture, "notify-oversized"]) as child:
            out, error = child.communicate(timeout=10)
            assert child.returncode == 0, (out, error)
        result = json.loads((root / "state/result.json").read_text())
        assert result["cause"] == "timed_out", result
        assert any("notification exceeded 16KiB bound" in error for error in result["monitor_errors"]), result
        try:
            forwarded = receiver.recv(16385)
        except BlockingIOError:
            pass
        else:
            raise AssertionError(("oversized packet forwarded a valid-looking prefix", forwarded))
        assert not (root / "state/notify.sock").exists(), "failed relay socket not released"

for force in (False, True):
    with tempfile.TemporaryDirectory(prefix="madrillo-notify-full-") as tmp:
        root = Path(tmp)
        target = root / "host.sock"
        with (socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as receiver,
              contextlib.ExitStack() as fillers):
            receiver.bind(str(target))
            receiver.setblocking(False)
            # Fill the real kernel receive queue and prove backpressure before
            # launch. An existing sender can exhaust its own buffer first, so
            # require EAGAIN on a fresh sender with no outstanding datagrams.
            queued = 0
            for _ in range(64):
                filler = fillers.enter_context(socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM))
                filler.connect(str(target))
                filler.setblocking(False)
                sent = 0
                while queued < 4096:
                    try:
                        filler.send(b"existing")
                        queued += 1
                        sent += 1
                    except BlockingIOError:
                        break
                else:
                    raise AssertionError("notification receive queue exceeds test bound")
                if sent == 0:
                    break
            else:
                raise AssertionError("notification receive queue did not reach backpressure")
            assert queued > 0
            parent, initial = socket.socketpair()
            with parent, initial:
                parent.settimeout(10)
                command = [monitor, "--state-dir", str(root / "state"), "--sdnotify-socket",
                           str(target), "--control", str(initial.fileno()), "--timeout",
                           "5" if force else "1", "--", fixture, "notify"]
                started = time.monotonic()
                with running(command, pass_fds=(initial.fileno(),)) as child:
                    initial.close()
                    assert message(parent)["type"] == "prepared"
                    assert message(parent)["type"] == "started"
                    output = bytearray()
                    while b"notification sent\n" not in output:
                        report = message(parent)
                        assert report["type"] == "output", report
                        assert report["stream"] == "stdout", report
                        output.extend(report["bytes"])
                    if force:
                        # The fixture confirms its packet was sent. Notification
                        # pressure must leave the control channel responsive.
                        request = json.dumps({"version": 1, "message": {"type": "terminate"}}).encode()
                        parent.sendall(struct.pack(">I", len(request)) + request)
                    completed = message(parent)
                    assert completed["type"] == "completed", completed
                    out, error = child.communicate(timeout=5)
                    assert child.returncode == 0, (out, error)
                assert time.monotonic() - started < 5, "notification pressure stalled termination"
                result = json.loads((root / "state/result.json").read_text())
                assert completed["result"] == result
                assert result["cause"] == ("forced" if force else "timed_out"), result
                assert any("notification relay: final delivery deadline expired" in error
                           for error in result["monitor_errors"]), result
                assert not (root / "state/notify.sock").exists(), "blocked relay socket not released"
            # Neither a partial notification nor an invented readiness event
            # may replace the datagrams that were already queued.
            for _ in range(queued):
                assert receiver.recv(16385) == b"existing"
            try:
                unexpected = receiver.recv(16385)
            except BlockingIOError:
                pass
            else:
                raise AssertionError(("unexpected delivery after relay timeout", unexpected))

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
print("notify qualification: filtering, oversized rejection, backpressure with force/timeout, socket release, and real Type=notify attribution passed")
