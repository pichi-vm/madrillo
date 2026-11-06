#!/usr/bin/env python3
"""Run real cgroup-v1 qualification in a disposable, diskless Linux VM.

The host's controllers and mounts are never changed. Requires qemu-system-x86_64,
cpio, and native monitor/fixture binaries. Supply an x86_64 Linux kernel with
MEMCG_V1, or let Ubuntu's apt download and extract its generic kernel package.
Uses KVM when available and QEMU's CPU emulator otherwise. All generated files
are removed on both success and failure. This is a test harness, not a monitor.
"""
import argparse
import gzip
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import sysconfig
import tempfile


def libraries(binary):
    result = subprocess.run(["ldd", str(binary)], text=True, capture_output=True)
    if result.returncode and "not a dynamic executable" not in result.stderr:
        raise RuntimeError(f"ldd {binary}: {result.stderr or result.stdout}")
    for line in result.stdout.splitlines():
        if "not found" in line:
            raise RuntimeError(f"missing dependency of {binary}: {line}")
        match = re.search(r"(?:=>\s*)?(/\S+)", line)
        if match:
            yield Path(match.group(1))


def rootfs(root, monitor, fixture):
    copied = set()

    def copy(binary, destination=None):
        destination = destination or binary
        target = root / str(destination).lstrip("/")
        if target not in copied:
            copied.add(target)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(binary, target)
            for library in libraries(binary):
                copy(library)

    python = Path(sys.executable).resolve()
    copy(python, Path("/usr/bin/python3"))
    for name in ["mount", "poweroff"]:
        source = shutil.which(name)
        if source is None:
            raise RuntimeError(f"{name} is required")
        copy(Path(source).resolve(), Path("/bin") / name)
    copy(monitor, Path("/bin/madrillo"))
    copy(fixture, Path("/bin/madrillo-fixture"))
    stdlib = Path(sysconfig.get_path("stdlib"))
    destination = root / stdlib.relative_to("/")
    shutil.copytree(stdlib, destination, ignore=shutil.ignore_patterns(
        "site-packages", "dist-packages", "__pycache__", "test", "tests",
        "idlelib", "tkinter", "turtledemo", "ensurepip"))
    # Extension modules can live under lib64 independently of the stdlib.
    extensions = Path(sysconfig.get_config_var("DESTSHARED"))
    extension_target = root / extensions.relative_to("/")
    shutil.copytree(extensions, extension_target, dirs_exist_ok=True)
    for extension in extension_target.glob("*.so"):
        for library in libraries(extension):
            copy(library)
    shutil.copy2(Path(__file__).with_name("check_cgroup.py"), root / "check.py")
    init = root / "init"
    init.write_text("""#!/usr/bin/python3 -S
import os
from pathlib import Path
import subprocess
import traceback

try:
    for path in ['/proc', '/sys', '/dev', '/tmp', '/run', '/sys/fs/cgroup']:
        Path(path).mkdir(parents=True, exist_ok=True)
    for kind, path in [('proc', '/proc'), ('sysfs', '/sys'), ('devtmpfs', '/dev')]:
        subprocess.run(['/bin/mount', '-t', kind, kind, path], check=True)
    Path('/dev/pts').mkdir(exist_ok=True)
    subprocess.run(['/bin/mount', '-t', 'devpts', 'devpts', '/dev/pts'], check=True)
    Path('/sys/fs/cgroup').mkdir(exist_ok=True)
    subprocess.run(['/bin/mount', '-t', 'cgroup', '-o', 'memory', 'memory',
                    '/sys/fs/cgroup'], check=True)
    subprocess.run(['/usr/bin/python3', '-S', '/check.py', '/bin/madrillo',
                    '/bin/madrillo-fixture', '/sys/fs/cgroup'], check=True)
    print('MADRILLO_CGROUP_V1_PASS', flush=True)
except BaseException:
    traceback.print_exc()
    print('MADRILLO_CGROUP_V1_FAIL', flush=True)
finally:
    os.sync()
    subprocess.run(['/bin/poweroff', '-f'], check=True)
""")
    init.chmod(0o755)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("monitor", type=Path)
    parser.add_argument("fixture", type=Path)
    parser.add_argument("kernel", type=Path, nargs="?")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="madrillo-cgroup-v1-") as temporary:
        directory = Path(temporary)
        kernel = args.kernel
        if kernel is None:
            # Download and extract only: no host kernel installation, initramfs
            # regeneration, bootloader changes or firmware packages are needed.
            dependencies = subprocess.check_output(
                ["apt-cache", "depends", "linux-image-generic"], text=True)
            package = next(line.split()[-1] for line in dependencies.splitlines()
                           if re.match(r"\s*Depends: linux-image-\d", line))
            subprocess.run(["apt-get", "download", package], cwd=directory, check=True)
            deb, = directory.glob("*.deb")
            subprocess.run(["dpkg-deb", "-x", str(deb), str(directory / "kernel")], check=True)
            kernel, = (directory / "kernel/boot").glob("vmlinuz-*")
        root = directory / "root"
        root.mkdir()
        rootfs(root, args.monitor.resolve(), args.fixture.resolve())
        names = b"\0".join(os.fsencode(str(p.relative_to(root)))
                            for p in sorted(root.rglob("*"))) + b"\0"
        archive = subprocess.run(["cpio", "--null", "-o", "-H", "newc", "--owner=0:0"],
                                 input=names, cwd=root, capture_output=True, check=True)
        initramfs = directory / "initramfs.gz"
        with gzip.open(initramfs, "wb", compresslevel=1) as output:
            output.write(archive.stdout)
        command = ["qemu-system-x86_64", "-nodefaults", "-no-reboot", "-nographic",
                   "-serial", "stdio", "-monitor", "none", "-m", "768", "-smp", "2",
                   "-kernel", str(kernel.resolve()), "-initrd", str(initramfs),
                   "-append", "console=ttyS0 rdinit=/init panic=-1 cgroup_no_v1= "
                              "random.trust_cpu=on"]
        if os.access("/dev/kvm", os.R_OK | os.W_OK):
            command.extend(["-enable-kvm", "-cpu", "host"])
        result = subprocess.run(command, text=True, stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, timeout=180)
        if result.returncode or "MADRILLO_CGROUP_V1_PASS" not in result.stdout:
            print(result.stdout, end="")
            raise RuntimeError(f"cgroup v1 VM failed: {result.returncode}")
        for line in result.stdout.splitlines():
            if "cgroup v1 qualification:" in line or "MADRILLO_CGROUP_V1_PASS" in line:
                print(line, flush=True)


if __name__ == "__main__":
    main()
