#!/usr/bin/env python3
"""Real Linux cgroup qualification. Requires a delegated writable parent.

Creates only temporary children, changes no ancestor limits, and always kills
and removes its own groups. No skipped cases: missing delegation/controllers
fails this qualification (the ordinary cargo suite remains usable without it).
"""
import contextlib
import json
import os
import signal
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import uuid

monitor, fixture = (str(Path(p).resolve()) for p in sys.argv[1:3])
parent = Path(sys.argv[3])
legacy = (parent / "memory.oom_control").exists()


def populated(path):
    if not legacy:
        return "populated 1" in (path / "cgroup.events").read_text()
    return any((group / "cgroup.procs").read_text().strip()
               for group in [path, *path.rglob("*")]
               if group.is_dir())


@contextlib.contextmanager
def scope():
    path = parent / f"madrillo-test-{os.getpid()}-{uuid.uuid4().hex}"
    path.mkdir()
    try:
        yield path
    finally:
        if path.exists():
            if not legacy:
                (path / "cgroup.kill").write_text("1")
            deadline = time.monotonic() + 5
            while populated(path):
                if time.monotonic() >= deadline:
                    raise RuntimeError(f"test scope did not empty: {path}")
                if legacy:
                    for group in [path, *(p for p in path.rglob("*") if p.is_dir())]:
                        for number in (group / "cgroup.procs").read_text().split():
                            try:
                                fd = os.pidfd_open(int(number))
                                try:
                                    signal.pidfd_send_signal(fd, signal.SIGKILL)
                                finally:
                                    os.close(fd)
                            except ProcessLookupError:
                                pass
                time.sleep(0.01)
            for child in sorted((p for p in path.rglob("*") if p.is_dir()),
                                key=lambda p: len(p.parts), reverse=True):
                child.rmdir()
            path.rmdir()


def run(root, group, mode, args=(), flags=(), success=True):
    state = root / "state"
    result = subprocess.run(
        [monitor, "--state-dir", str(state), "--cgroup", str(group),
         "--timeout", "5", *flags, "--", fixture, mode, *map(str, args)],
        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        timeout=12,
    )
    assert (result.returncode == 0) == success, result.stderr
    return json.loads((state / "result.json").read_text())


for terminal in (False, True):
    with scope() as group, tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        # Probe is the first workload code: placement must already have happened.
        result = run(root, group, "placement", [group.name], ["--terminal"] if terminal else [])
        assert result["leader"] == {"type": "exit", "code": 0}, result
        assert result["cleanup"] == {"type": "complete"}, result
        assert not result["log_errors"] and not result["output_truncated"], result
        assert result["oom"] == {"type": "none_observed"}, result

    for mode in ["escaped-tree", "nested-cgroup-tree"]:
        with scope() as group, tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            heartbeat = root / "heartbeat"
            result = run(root, group, mode, [heartbeat, group], ["--terminal"] if terminal else [])
            assert result["leader"] == {"type": "exit", "code": 37}, result
            assert result["cleanup"] == {"type": "complete"}, result
            before = heartbeat.read_bytes()
            assert before
            time.sleep(0.15)
            assert heartbeat.read_bytes() == before, "setsid descendant survived"
            assert not populated(group)
            assert result["oom"] == {"type": "none_observed"}, result

with scope() as group, tempfile.TemporaryDirectory() as tmp:
    # A 32MiB limit and no swap apply only to this new, empty test cgroup.
    # Fixture itself has a 64MiB allocation ceiling if enforcement is broken.
    if legacy:
        (group / "memory.limit_in_bytes").write_text(str(32 * 1024 * 1024))
        (group / "memory.memsw.limit_in_bytes").write_text(str(32 * 1024 * 1024))
    else:
        (group / "memory.max").write_text(str(32 * 1024 * 1024))
        (group / "memory.swap.max").write_text("0")
    result = run(Path(tmp), group, "bounded-oom")
    assert result["leader"] == {"type": "signal", "signal": 9, "core_dumped": False}, result
    assert result["cleanup"] == {"type": "complete"}, result
    assert result["oom"]["type"] == "observed" and result["oom"]["kills"] >= 1, result
    assert result["oom"]["events"] >= 1, result

with scope() as group, scope() as replacement, tempfile.TemporaryDirectory() as tmp:
    result = run(Path(tmp), group, "remove-cgroup", [group, replacement])
    assert result["leader"] == {"type": "exit", "code": 0}, result
    assert result["oom"]["type"] == "unavailable", result
    assert result["cleanup"]["type"] == "incomplete", result
    assert not group.exists(), "fixture did not remove the monitored cgroup"

with tempfile.TemporaryDirectory() as tmp:
    root = Path(tmp)
    fake = root / "fake-cgroup"
    fake.mkdir()
    (fake / "cgroup.events").write_text("populated 0\n")
    (fake / "cgroup.kill").write_text("sentinel")
    result = run(root, fake, "emit", success=False)
    assert result["leader"]["type"] == "spawn_failed", result
    assert (fake / "cgroup.kill").read_text() == "sentinel"

print(f"cgroup v{1 if legacy else 2} qualification: pre-exec placement + escaped descendants with pipe/PTY; bounded OOM evidence; removal is not OOM; fake-filesystem rejection passed", flush=True)
