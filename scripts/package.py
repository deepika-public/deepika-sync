#!/usr/bin/env python3
"""Build and package the daemon for one target; no upload.

Without arguments: the native Linux x86_64 (glibc) archive the GitLab release publishes.
With `--target T`: the archive for T, as GitHub Actions builds it on each platform.
"""
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
os.chdir(ROOT)
version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
    raise SystemExit("Packaging requires a stable X.Y.Z version")
tag = f"v{version}"
# The tag being released, on GitLab or on GitHub; none for a branch build.
release_tag = os.environ.get("CI_COMMIT_TAG") or (
    os.environ.get("GITHUB_REF_NAME") if os.environ.get("GITHUB_REF_TYPE") == "tag" else None)
if release_tag and release_tag != tag:
    raise SystemExit("Git tag must match Cargo.toml version")
rust = subprocess.check_output(["rustc", "-vV"], text=True)
host = re.search(r"^host: (.+)$", rust, re.M)[1]
TARGETS = ("x86_64-unknown-linux-gnu", "x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl",
           "aarch64-apple-darwin", "x86_64-apple-darwin")
if sys.argv[1:2] == ["--target"] and len(sys.argv) == 3:
    target = sys.argv[2]
elif len(sys.argv) == 1:
    target = host
    if host != "x86_64-unknown-linux-gnu":
        raise SystemExit("Without --target, only native x86_64-unknown-linux-gnu packaging is validated")
else:
    raise SystemExit("Usage: package.py [--target TRIPLE]")
if target not in TARGETS:
    raise SystemExit(f"Unsupported target {target}; expected one of {', '.join(TARGETS)}")
# Force the target so Cargo configuration cannot silently package another platform.
subprocess.run(["cargo", "build", "--locked", "--release", "--target", target], check=True)
metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"]))
binary = Path(metadata["target_directory"]) / target / "release/deepika-sync"
try:
    announced = subprocess.check_output([str(binary), "--version"], text=True).strip()
except OSError:
    # An Intel binary built on Apple Silicon only runs under Rosetta, absent from CI.
    if target == host:
        raise
    announced = None
    print(f"warning: {target} binary not run on {host}; its version is not checked", file=sys.stderr)
if announced is not None and announced != f"deepika-sync {version}":
    raise SystemExit("Binary version mismatch")
glibc = None
if target.endswith("-gnu"):
    versions = re.findall(r"GLIBC_([0-9]+\.[0-9]+)", subprocess.check_output(["readelf", "--version-info", str(binary)], text=True))
    glibc = max(versions, key=lambda s: tuple(map(int, s.split("."))))
commit = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
epoch = int(os.environ.get("SOURCE_DATE_EPOCH") or subprocess.check_output(["git", "show", "-s", "--format=%ct", "HEAD"], text=True))
# Read from the source, so the archive can never announce a protocol the binary does not speak.
alpn = re.search(r'pub const ALPN: &\[u8\] = b"([^"]+)";', (ROOT / "src/transport.rs").read_text()).group(1)
info = {"component": "deepika-sync", "version": version, "commit": commit,
        "dirty": bool(subprocess.check_output(["git", "status", "--porcelain"])),
        "target": target, "glibc_required": glibc, "rustc": rust.strip(),
        "editor_protocols": ["y-sync over WebSocket", "LSP 3.17 over stdio"], "network_protocol": alpn}
if release_tag and info["dirty"]:
    raise SystemExit("Refusing to package a modified release checkout")
out = ROOT / "dist"
out.mkdir(exist_ok=True)
name = f"deepika-sync-{tag}-{target}"
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
print(f"{archive} (requires glibc >= {glibc})" if glibc else f"{archive} (static, no glibc requirement)" if "musl" in target else str(archive))
