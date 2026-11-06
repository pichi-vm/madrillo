#!/usr/bin/env python3
"""Check the actual default library graph and unsafe-code policy on every target."""
import json
from pathlib import Path
import re
import subprocess
import tomllib

root = Path(__file__).resolve().parent.parent
manifest = tomllib.loads((root / "Cargo.toml").read_text())
assert manifest["features"]["default"] == [], "default features must stay empty"
host = next(line.removeprefix("host: ") for line in
            subprocess.check_output(["rustc", "-vV"], text=True).splitlines()
            if line.startswith("host: "))
metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--locked", "--format-version", "1",
     "--no-default-features", "--filter-platform", host], cwd=root, text=True))
package = next(p for p in metadata["packages"] if Path(p["manifest_path"]) == root / "Cargo.toml")
nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
packages = {p["id"]: p for p in metadata["packages"]}
direct = [dependency for dependency in nodes[package["id"]]["deps"]
          if any(kind["kind"] in (None, "build") for kind in dependency["dep_kinds"])]
assert len(direct) == 1 and packages[direct[0]["pkg"]]["name"] == "serde", direct
assert "cli" not in nodes[package["id"]]["features"], "CLI leaked into default graph"
assert manifest["lints"]["rust"]["unsafe_code"] == "deny"
for target in package["targets"]:
    source = Path(target["src_path"])
    policy = "deny" if source == root / "src/main.rs" else "forbid"
    assert f"#![{policy}(unsafe_code)]" in source.read_text(), f"missing policy: {source}"
main = (root / "src/main.rs").read_text()
exception = '#[cfg(windows)]\n#[allow(unsafe_code, reason = "User-approved, audited Windows OS boundary")]\nmod windows;'
assert main.count(exception) == 1, "the sole exception must name the private Windows module"
for source in (root / "src").rglob("*.rs"):
    text = source.read_text().replace(exception, "") if source.name == "main.rs" else source.read_text()
    assert not re.search(r"#\s*!?\s*\[\s*(?:allow|expect)\s*\([^)]*\bunsafe_code\b", text), source
library = (root / "src/lib.rs").read_text()
# This small public module is reviewed as declarations and serde derives only.
# Compiler/Clippy checks enforce Rust validity and production panic policy.
assert "\nimpl " not in library and "\nfn " not in library and "\npub fn " not in library
assert "\nmod " not in library and "\npub mod " not in library
print("bare library audit: serde-only normal/build graph; empty defaults; sole private Windows unsafe boundary")
