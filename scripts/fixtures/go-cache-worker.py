#!/usr/bin/env python3
"""Exercise a candidate Go cache worker using only newly generated, bounded fixtures."""
import argparse
import json
import os
from pathlib import Path
import select
import subprocess
import tempfile
import time


def identity(path):
    stat = os.lstat(path)
    return {"device": f"{stat.st_dev:016x}", "inode": f"{stat.st_ino:016x}",
            "high": "0000000000000000", "birth": None}


def request(cache):
    return {"binding": {"path": {"encoding": "unix", "units": list(os.fsencode(cache))},
                        "parent_identity": identity(cache.parent), "identity": identity(cache)},
            "operation": {"identity": {"operation_id": "generated-go-cache-fixture", "generation": 1},
                          "deadline_monotonic_ns": time.monotonic_ns() + 30_000_000_000,
                          "output_bytes": 65536, "line_bytes": 65536,
                          "first_output_timeout_ms": 10000, "idle_timeout_ms": 5000}}


def run(binary, cancel):
    with tempfile.TemporaryDirectory(prefix="spaceprune-go-native-") as directory:
        base = Path(directory)
        cache = base / "cache"
        cache.mkdir()
        count = 800 if cancel else 2
        for number in range(count):
            module = cache / f"module-{number}"
            module.mkdir()
            (module / "source.go").write_bytes(b"package module\n")
            (module / "source.go").chmod(0o444)
            module.chmod(0o555)
        external = base / "external"
        external.mkdir()
        (external / "keep").write_bytes(b"external target")
        external.chmod(0o555)
        (cache / "external-link").symlink_to(external, target_is_directory=True)
        cache.chmod(0o555)
        held = os.open(cache / "module-0", os.O_RDONLY | os.O_DIRECTORY)
        process = None
        try:
            process = subprocess.Popen([str(binary), "--spaceprune-go-cache-capture-v1"],
                                       stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                       stderr=subprocess.PIPE, text=True)
            process.stdin.write(json.dumps(request(cache)) + "\n")
            process.stdin.flush()
            assert select.select([process.stdout], [], [], 10)[0], "worker startup timeout"
            first = json.loads(process.stdout.readline())
            assert first["kind"] == "ready", first
            if cancel:
                deadline = time.monotonic() + 5
                while cache.exists() and time.monotonic() < deadline:
                    time.sleep(0.0005)
                assert not cache.exists(), "cancellation must observe the actual root capture"
                process.stdin.close()
                process.stdin = None
            else:
                # Keep the cancellation pipe open until the terminal report arrives.
                assert select.select([process.stdout], [], [], 10)[0], "worker response timeout"
            if cancel:
                output, errors = process.communicate(timeout=10)
                replies = [json.loads(line) for line in output.splitlines()]
            else:
                replies = []
                while True:
                    reply = json.loads(process.stdout.readline())
                    replies.append(reply)
                    if reply["kind"] == "finished":
                        break
                process.stdin.close()
                process.stdin = None
                _, errors = process.communicate(timeout=10)
            assert process.returncode == 0, errors
            report = next(reply["report"] for reply in replies if reply["kind"] == "finished")
            assert os.fstat(held).st_mode & 0o777 == 0o555
            assert external.stat().st_mode & 0o777 == 0o555
            assert (external / "keep").read_bytes() == b"external target"
            assert not list(base.glob(".spaceprune-execute-*")), "unexpected detached recovery contents"
            if cancel:
                assert report["error"]["classification"]["kind"] == "cancelled", report
                assert cache.is_dir(), report
                assert cache.stat().st_mode & 0o777 == 0o555
                assert report["removed_entries"] == 0, report
                assert len(list(cache.glob("module-*"))) == count
            else:
                assert report["error"] is None, report
                assert report["reclaimed_estimate"] == 15 * count, report
                assert not cache.exists()
            print(json.dumps({"binary": str(binary), "mode": "cancel" if cancel else "ordinary",
                              "report": report}, ensure_ascii=False))
        finally:
            os.close(held)
            if process is not None and process.poll() is None:
                process.kill()
                process.wait()
            # Restore only this generated fixture's modes after acceptance checks.
            for path, dirs, _ in os.walk(base):
                os.chmod(path, 0o700)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--cancel", action="store_true")
    args = parser.parse_args()
    if os.geteuid() == 0:
        parser.error("run this ordinary-worker fixture as a non-root account")
    run(args.binary.resolve(strict=True), args.cancel)


if __name__ == "__main__":
    main()
