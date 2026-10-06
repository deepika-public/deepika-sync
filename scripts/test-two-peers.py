#!/usr/bin/env python3
"""E2E test: two real deepika-sync daemon processes collaborating via Iroh QUIC.

Tests:
1. Peer A shares a workspace and generates an invite code.
2. Peer B joins using the invite code.
3. Peer A creates a file on disk; Peer B automatically receives and projects it.
4. Peer B edits the file on disk; Peer A automatically receives the edit.
5. Verification of CLI `documents` and `status` commands.
6. Clean termination.
"""
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
    if not pathlib.Path(BIN).exists():
        print(f"Building {BIN}...")
        subprocess.run(["cargo", "build"], check=True)

    processes = []
    try:
        with tempfile.TemporaryDirectory(prefix="deepika-sync-e2e-") as td:
            root_a = pathlib.Path(td) / "alice"
            root_b = pathlib.Path(td) / "bob"
            root_a.mkdir()
            root_b.mkdir()

            print("1. Starting Alice (deepika-sync share)...")
            log_a = open(root_a / "daemon_stderr.log", "w")
            proc_a = subprocess.Popen(
                [BIN, "share", str(root_a), "--ws-port", "49180"],
                stdout=subprocess.PIPE,
                stderr=log_a,
                text=True,
            )
            processes.append(proc_a)

            # Read Alice's invitation code from stdout
            invite_code = None
            start_time = time.monotonic()
            while time.monotonic() - start_time < 10:
                line = proc_a.stdout.readline()
                if "Code d'invitation :" in line:
                    invite_code = line.split(":", 1)[1].strip()
                    break
            assert invite_code, "Failed to get invite code from Alice"
            print(f"   Alice invite code: {invite_code[:32]}...")

            print("2. Starting Bob (deepika-sync join)...")
            log_b = open(root_b / "daemon_stderr.log", "w")
            proc_b = subprocess.Popen(
                [BIN, "join", invite_code, "--root", str(root_b), "--ws-port", "49181", "--yes"],
                stdout=subprocess.PIPE,
                stderr=log_b,
                text=True,
            )
            processes.append(proc_b)

            start_time = time.monotonic()
            bob_started = False
            while time.monotonic() - start_time < 10:
                line = proc_b.stdout.readline()
                if "Session ouverte" in line:
                    bob_started = True
                    break
            assert bob_started, "Bob failed to start"
            print("   Bob session opened successfully!")

            time.sleep(1.0)

            print("3. Testing file creation: Alice writes note.md...")
            (root_a / "note.md").write_text("# Hello from Alice\nInitial content\n")

            # Wait for Bob to receive the projected file
            print("   Waiting for Bob to receive note.md...")
            wait_until(lambda: (root_b / "note.md").exists() and "Initial content" in (root_b / "note.md").read_text())
            print("   Bob received note.md with correct content!")

            print("4. Testing bidirectional edit: Bob edits note.md...")
            time.sleep(0.5)
            (root_b / "note.md").write_text("# Hello from Alice\nInitial content\nAdded by Bob\n")

            # Wait for Alice to receive Bob's edit
            print("   Waiting for Alice to receive Bob's update...")
            wait_until(lambda: "Added by Bob" in (root_a / "note.md").read_text())
            print("   Alice received Bob's edit!")

            print("5. Inspecting Bob while his daemon runs (no session lock needed)...")
            live = subprocess.check_output([BIN, "documents", "--root", str(root_b)], text=True)
            assert "note.md" in live, f"note.md not in live documents: {live}"

            print("6. Stopping Bob to verify offline CLI storage queries...")
            proc_b.terminate()
            proc_b.wait(timeout=3)

            docs_out = subprocess.check_output([BIN, "documents", "--root", str(root_b)], text=True)
            assert "note.md" in docs_out, f"note.md not in documents: {docs_out}"

            status_out = subprocess.check_output([BIN, "status", "--root", str(root_b)], text=True)
            assert "documents_count" in status_out, f"invalid status output: {status_out}"
            print("   CLI documents & status verified on SQLite storage!")

            print("\nPASS: Two-peer synchronization, Iroh QUIC, debounced projection, and CLI OK!")
    except Exception as e:
        print(f"\n--- ERROR: {e} ---")
        for name, root in [("Alice", root_a), ("Bob", root_b)]:
            log_f = root / ".collab" / "daemon.log"
            if log_f.exists():
                print(f"\n=== {name} daemon.log ===")
                print(log_f.read_text())
            stderr_f = root / "daemon_stderr.log"
            if stderr_f.exists():
                content = stderr_f.read_text().strip()
                if content:
                    print(f"\n=== {name} stderr ===")
                    print(content)
        raise
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
