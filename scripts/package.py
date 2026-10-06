#!/usr/bin/env python3
"""Build and package the native Linux x86_64 daemon; no upload."""
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
os.chdir(ROOT)
version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
    raise SystemExit("Packaging requires a stable X.Y.Z version")
tag = f"v{version}"
if os.environ.get("CI_COMMIT_TAG") and os.environ["CI_COMMIT_TAG"] != tag:
    raise SystemExit("Git tag must match Cargo.toml version")
rust = subprocess.check_output(["rustc", "-vV"], text=True)
host = re.search(r"^host: (.+)$", rust, re.M)[1]
if host != "x86_64-unknown-linux-gnu":
    raise SystemExit("Only native x86_64-unknown-linux-gnu packaging is validated")
# Force the target so Cargo configuration cannot silently package another platform.
subprocess.run(["cargo", "build", "--locked", "--release", "--target", host], check=True)
metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"]))
binary = Path(metadata["target_directory"]) / host / "release/deepika-sync"
if subprocess.check_output([str(binary), "--version"], text=True).strip() != f"deepika-sync {version}":
    raise SystemExit("Binary version mismatch")
versions = re.findall(r"GLIBC_([0-9]+\.[0-9]+)", subprocess.check_output(["readelf", "--version-info", str(binary)], text=True))
glibc = max(versions, key=lambda s: tuple(map(int, s.split("."))))
commit = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
epoch = int(os.environ.get("SOURCE_DATE_EPOCH") or subprocess.check_output(["git", "show", "-s", "--format=%ct", "HEAD"], text=True))
# Read from the source, so the archive can never announce a protocol the binary does not speak.
alpn = re.search(r'pub const ALPN: &\[u8\] = b"([^"]+)";', (ROOT / "src/transport.rs").read_text()).group(1)
info = {"component": "deepika-sync", "version": version, "commit": commit,
        "dirty": bool(subprocess.check_output(["git", "status", "--porcelain"])),
        "target": host, "glibc_required": glibc, "rustc": rust.strip(),
        "editor_protocols": ["y-sync over WebSocket", "LSP 3.17 over stdio"], "network_protocol": alpn}
if os.environ.get("CI_COMMIT_TAG") and info["dirty"]:
    raise SystemExit("Refusing to package a modified release checkout")
out = ROOT / "dist"
out.mkdir(exist_ok=True)
name = f"deepika-sync-{tag}-{host}"
archive = out / f"{name}.tar.gz"
with tempfile.TemporaryDirectory(prefix="deepika-sync-package-") as temp:
    stage = Path(temp) / name
    stage.mkdir()
    shutil.copy2(binary, stage / "deepika-sync")
    shutil.copy2(ROOT / "README.md", stage / "README.md")
    shutil.copytree(ROOT / "docs", stage / "docs")
    (stage / "BUILD.json").write_text(json.dumps(info, indent=2) + "\n")
    with archive.open("wb") as raw, gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as zipped, tarfile.open(fileobj=zipped, mode="w") as tar:
        for path in sorted(stage.rglob("*")):
            entry = tar.gettarinfo(str(path), arcname=f"{name}/{path.relative_to(stage)}")
            entry.uid = entry.gid = 0
            entry.uname = entry.gname = ""
            entry.mtime = epoch
            entry.mode = 0o755 if path.is_dir() or path.name == "deepika-sync" else 0o644
            if path.is_file():
                with path.open("rb") as source:
                    tar.addfile(entry, source)
            else:
                tar.addfile(entry)
(out / "SHA256SUMS").write_text(f"{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}\n")
print(f"{archive} (requires glibc >= {glibc})")
