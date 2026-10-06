#!/usr/bin/env python3
"""Multi-peer mesh test: 3 real deepika-sync daemons collaborating concurrently via Iroh QUIC."""
import os
import pathlib
import subprocess
import tempfile
import time

BIN = os.environ.get("DEEPIKA_SYNC_BINARY") or str(
    pathlib.Path(__file__).resolve().parents[1] / "target/debug/deepika-sync"
)


def wait_until(fn, timeout=15, interval=0.1):
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


def main():
    processes = []
    try:
        with tempfile.TemporaryDirectory(prefix="deepika-sync-mesh-") as td:
            roots = [pathlib.Path(td) / f"peer_{i}" for i in range(3)]
            for r in roots:
                r.mkdir()

            print("1. Starting Peer 0 (Leader)...")
            log0 = open(roots[0] / "daemon.log", "w")
            p0 = subprocess.Popen(
                [BIN, "share", str(roots[0]), "--ws-port", "49210"],
                stdout=subprocess.PIPE,
                stderr=log0,
                text=True,
            )
            processes.append(p0)

            invite_code = None
            start_t = time.monotonic()
            while time.monotonic() - start_t < 10:
                line = p0.stdout.readline()
                if "Code d'invitation :" in line:
                    invite_code = line.split(":", 1)[1].strip()
                    break
            assert invite_code, "Failed to get invite code"

            print("2. Joining Peer 1 and Peer 2 to the session...")
            for i in [1, 2]:
                log = open(roots[i] / "daemon.log", "w")
                p = subprocess.Popen(
                    [BIN, "join", invite_code, "--root", str(roots[i]), "--ws-port", str(49210 + i), "--yes"],
                    stdout=subprocess.PIPE,
                    stderr=log,
                    text=True,
                )
                processes.append(p)

            time.sleep(1.5)

            print("3. Peer 0 creates shared document 'mesh.md'...")
            (roots[0] / "mesh.md").write_text("# Shared Mesh Document\n")

            print("   Waiting for Peer 1 and Peer 2 to receive 'mesh.md'...")
            for i in [1, 2]:
                wait_until(lambda r=roots[i]: (r / "mesh.md").exists())
            print("   All 3 peers have 'mesh.md' projected on disk!")

            print("4. Peer 1 contributes an addition...")
            (roots[1] / "mesh.md").write_text("# Shared Mesh Document\nContribution from Peer 1\n")

            print("   Waiting for Peer 0 and Peer 2 to converge on Peer 1's edit...")
            for i in [0, 2]:
                wait_until(lambda r=roots[i]: "Contribution from Peer 1" in (r / "mesh.md").read_text())
            print("   Mesh convergence verified across all nodes!")

            print("\nPASS: 3-peer mesh collaborative synchronization OK!")
    finally:
        for p in processes:
            if p.poll() is None:
                p.terminate()
                try:
                    p.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    p.kill()


if __name__ == "__main__":
    main()
