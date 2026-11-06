#!/usr/bin/env python3
"""Build the committed source RPM and native binary RPM in temporary directories."""
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent


def run(command, directory):
    print("RUN", " ".join(command), flush=True)
    with (directory / "build.log").open("w+") as log:
        result = subprocess.run(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT,
                                timeout=900, check=False)
        log.seek(0)
        output = log.read()
        if result.returncode:
            print("\n".join(output.splitlines()[-100:]))
            result.check_returncode()
        return output


with tempfile.TemporaryDirectory(prefix="madrillo-rpm-") as temporary:
    work = Path(temporary)
    source = work / "source"
    source.mkdir()
    scripts = work / "scripts"
    scripts.mkdir()
    top = work / "rpm"
    for name in ("BUILD", "BUILDROOT", "RPMS", "SOURCES", "SPECS", "SRPMS"):
        (top / name).mkdir(parents=True)
    run(["make", "-f", ".copr/Makefile", "srpm", "DNF=true", f"outdir={source}"], work)
    packages = list(source.glob("*.src.rpm"))
    assert len(packages) == 1, packages
    output = run(["rpmbuild", "--rebuild", str(packages[0]), "--define", f"_topdir {top}",
                  "--define", f"_tmppath {scripts}", "--define", "_smp_mflags -j4"], work)
    assert "Packaged monitor launch and retained completion passed" in output, output
    packages = list((top / "RPMS").glob("*/madrillo-[0-9]*.rpm"))
    assert len(packages) == 1, packages
    files = subprocess.check_output(["rpm", "-qlp", str(packages[0])], text=True).splitlines()
    assert "/usr/bin/madrillo" in files, files
    assert not any("madrillo-fixture" in file for file in files), files
    for required in ("LICENSE", "LICENSE.dependencies", "cargo-vendor.txt",
                     "rust/COPYRIGHT-library.html"):
        assert f"/usr/share/licenses/madrillo/{required}" in files, required
    assert any(file.startswith("/usr/share/licenses/madrillo/dependencies/")
               and file.endswith("LICENSE-MIT") for file in files), files
    print("Native RPM qualification passed: locked source, offline build, executable launch and license payload")
