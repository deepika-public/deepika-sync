#!/usr/bin/env python3
"""Reproducible benchmark measuring convergence latency, CPU, RSS, and storage in deepika-sync."""
import argparse
import hashlib
import json
import os
import pathlib
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument("--binary", default="target/debug/deepika-sync")
parser.add_argument("--size", type=int, default=1024)
parser.add_argument("--files", type=int, default=1)
parser.add_argument("--peers", type=int, default=2)
parser.add_argument("--edits", type=int, default=20)
parser.add_argument("--output", default=None)
parser.add_argument("--settle", type=float, default=1.0)
args = parser.parse_args()

binary = str(pathlib.Path(args.binary).resolve())
processes = []


def wait_until(fn, timeout=30, interval=0.01):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        try:
            res = fn()
            if res:
                return res
        except Exception:
            pass
        time.sleep(interval)
    raise TimeoutError(f"Condition not met within {timeout}s")


def get_counters(pid):
    try:
        stat = pathlib.Path(f"/proc/{pid}/stat").read_text().split()
        status = pathlib.Path(f"/proc/{pid}/status").read_text()
        io_lines = pathlib.Path(f"/proc/{pid}/io").read_text().splitlines()
        io = dict(line.split(": ") for line in io_lines)
        return {
            "cpu": (int(stat[13]) + int(stat[14])) / os.sysconf("SC_CLK_TCK"),
            "write_bytes": int(io.get("write_bytes", 0)),
            "rss_kib": int(status.split("VmRSS:")[1].split()[0]),
        }
    except Exception:
        return {"cpu": 0.0, "write_bytes": 0, "rss_kib": 0}


def quantiles(values):
    if not values:
        return {"p50_ms": 0.0, "p95_ms": 0.0, "p99_ms": 0.0}
    s = sorted(values)
    return {
        f"p{q}_ms": round(s[min(len(s) - 1, int((len(s) - 1) * q / 100))] * 1000, 3)
        for q in [50, 95, 99]
    }


try:
    with tempfile.TemporaryDirectory(prefix="deepika-sync-bench-") as td:
        roots = []
        invite_code = None

        for i in range(args.peers):
            root = pathlib.Path(td) / f"peer_{i}"
            root.mkdir()
            roots.append(root)

            # Only leader (Peer 0) initializes initial files
            if i == 0:
                for f in range(args.files):
                    (root / f"file_{f}.md").write_text("x" * args.size)

            log_file = open(root / "daemon.log", "w")
            port = 48000 + i * 10 + 5
            cmd = (
                [binary, "share", str(root), "--ws-port", str(port)]
                if i == 0
                else [
                    binary,
                    "join",
                    invite_code,
                    "--root",
                    str(root),
                    "--ws-port",
                    str(port),
                    "--yes",
                ]
            )
            p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=log_file, text=True)
            processes.append(p)

            if i == 0:
                start_t = time.monotonic()
                while time.monotonic() - start_t < 15:
                    line = p.stdout.readline()
                    if "Code d'invitation :" in line:
                        invite_code = line.split(":", 1)[1].strip()
                        break
                assert invite_code, "Peer 0 failed to generate invite code"

        # Wait for initial mesh synchronization
        time.sleep(1.5)
        for r in roots[1:]:
            wait_until(lambda r=r: (r / "file_0.md").exists())

        # Measure baseline / idle
        before_idle = [get_counters(p.pid) for p in processes]
        time.sleep(args.settle)
        idle = [get_counters(p.pid) for p in processes]

        # Benchmark edit propagation latency
        latencies = []
        target_file_0 = roots[0] / "file_0.md"
        base_content = "x" * args.size

        start_busy = time.monotonic()
        for edit_idx in range(args.edits):
            marker = f"edit_{edit_idx:04d}"
            content = f"{base_content}\n{marker}\n"
            t0 = time.monotonic()
            target_file_0.write_text(content)

            # Wait until all other peers receive the update on disk
            for r in roots[1:]:
                wait_until(
                    lambda r=r, m=marker: (r / "file_0.md").exists()
                    and m in (r / "file_0.md").read_text(),
                    timeout=10,
                )
            latencies.append(time.monotonic() - t0)

        busy_wall = time.monotonic() - start_busy
        after_busy = [get_counters(p.pid) for p in processes]

        sqlite_bytes = sum(
            (r / ".collab/state.sqlite").stat().st_size
            for r in roots
            if (r / ".collab/state.sqlite").exists()
        )

        result = {
            "binary_sha256": hashlib.sha256(pathlib.Path(binary).read_bytes()).hexdigest(),
            "peers": args.peers,
            "files": args.files,
            "file_size_bytes": args.size,
            "edits": args.edits,
            "latency": quantiles(latencies),
            "busy_wall_seconds": round(busy_wall, 4),
            "busy_cpu_seconds": round(
                sum(y["cpu"] - x["cpu"] for x, y in zip(idle, after_busy)), 4
            ),
            "idle_cpu_seconds": round(
                sum(y["cpu"] - x["cpu"] for x, y in zip(before_idle, idle)), 4
            ),
            "total_rss_kib": sum(x["rss_kib"] for x in after_busy),
            "sqlite_storage_bytes": sqlite_bytes,
        }

        print(json.dumps(result, indent=2))
        if args.output:
            pathlib.Path(args.output).write_text(json.dumps(result, indent=2) + "\n")

finally:
    for p in processes:
        if p.poll() is None:
            p.terminate()
            try:
                p.wait(timeout=2)
            except subprocess.TimeoutExpired:
                p.kill()
