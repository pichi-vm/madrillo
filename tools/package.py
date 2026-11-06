#!/usr/bin/env python3
"""Build a native binary and verified bare-library source package with licenses."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parent.parent


def output(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True)


def run(*args):
    subprocess.run(args, cwd=ROOT, check=True)


def main(destination):
    rust = output("rustc", "-vV")
    host = next(line.removeprefix("host: ") for line in rust.splitlines() if line.startswith("host: "))
    metadata = json.loads(output("cargo", "metadata", "--locked", "--format-version=1",
                                 "--features", "cli", "--filter-platform", host))
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    root = metadata["resolve"]["root"]
    selected = set()
    pending = [root]
    while pending:
        package = pending.pop()
        if package in selected:
            continue
        selected.add(package)
        pending.extend(dependency["pkg"] for dependency in nodes[package]["deps"]
                       if any(kind["kind"] != "dev" for kind in dependency["dep_kinds"]))
    version = packages[root]["version"]
    run("cargo", "build", "--locked", "--release", "--target", host, "--features", "cli", "--bin", "madrillo")
    # Cargo verifies the packaged sources with the default bare-library feature set.
    run("cargo", "package", "--locked", "--no-default-features")
    target = Path(metadata["target_directory"])
    executable = "madrillo.exe" if "windows" in host else "madrillo"
    binary = target / host / "release" / executable
    run(str(binary), "--version")
    source = target / "package" / f"madrillo-{version}.crate"
    destination.mkdir(parents=True, exist_ok=True)
    archive = destination / f"madrillo-{version}-{host}.zip"
    inventory = []
    with tempfile.TemporaryDirectory(prefix="madrillo-package-") as temporary:
        stage = Path(temporary)
        for path in (binary, source, ROOT / "LICENSE", ROOT / "README.md"):
            shutil.copy2(path, stage / path.name)
        for identity in sorted(selected - {root}):
            package = packages[identity]
            directory = Path(package["manifest_path"]).parent
            name = f'{package["name"]}-{package["version"]}'
            licenses = stage / "licenses" / name
            licenses.mkdir(parents=True)
            candidates = [path for path in directory.iterdir()
                          if path.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE", "COPYRIGHT"))]
            if package.get("license_file"):
                candidates.append(directory / package["license_file"])
            if not package.get("license") and not package.get("license_file"):
                raise ValueError(f"missing license declaration: {identity}")
            for path in set(candidates):
                if path.is_symlink() or not path.resolve().is_relative_to(directory.resolve()):
                    raise ValueError(f"license outside dependency source: {path}")
                if path.is_dir():
                    shutil.copytree(path, licenses / path.name)
                else:
                    shutil.copy2(path, licenses / path.name)
            if not any(licenses.iterdir()):
                raise ValueError(f"no license text packaged by dependency: {identity}")
            inventory.append({key: package[key] for key in ("name", "version", "source", "license", "license_file")})
        rust_docs = Path(output("rustc", "--print", "sysroot").strip()) / "share/doc/rust"
        shutil.copy2(rust_docs / "COPYRIGHT-library.html", stage / "licenses" / "rust-library.html")
        shutil.copytree(rust_docs / "licenses", stage / "licenses" / "rust")
        (stage / "dependencies.json").write_text(json.dumps({
            "revision": output("git", "rev-parse", "HEAD").strip(),
            "rust": rust, "target": host, "packages": inventory,
        }, indent=2) + "\n", encoding="utf-8")
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED, strict_timestamps=False) as bundle:
            for path in sorted(stage.rglob("*")):
                if path.is_file():
                    bundle.write(path, path.relative_to(stage))
    checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_suffix(".zip.sha256").write_text(f"{checksum}  {archive.name}\n", encoding="ascii")
    print(f"Packaged {host}: {archive}; {len(inventory)} dependency license entries")


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
