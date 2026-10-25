#!/usr/bin/python3
"""Strict disposable Docker CLI fixture; never connects to a Docker daemon."""
import json
import os
import pathlib
import sys

root = pathlib.Path(os.environ["SPACEPRUNE_DOCKER_FIXTURE"])
args = sys.argv[1:]
with (root / "commands.jsonl").open("a") as log:
    log.write(json.dumps(args) + "\n")
state = json.loads((root / "engine.json").read_text())
state.setdefault("images", ["sha256:" + "a" * 64, "sha256:" + "b" * 64])
state.setdefault("stopped", ["c" * 64])
if args[:2] == ["context", "inspect"]:
    print(json.dumps([{"Endpoints": {"docker": {"Host": state["endpoint"]}}}]))
    sys.exit(0)
assert args[:2] == ["--host", "unix:///fixture/docker.sock"], args
args = args[2:]
if args[0] == "info":
    if state.get("info_error"):
        sys.stderr.write(state["info_error"])
        sys.exit(1)
    print(json.dumps(state["engine"]))
elif args[-1] == "--help":
    if args[0] == "buildx" and state.get("buildx_missing"):
        sys.stderr.write("docker: 'buildx' is not a docker command")
        sys.exit(1)
    print("--format --builder --filter --no-trunc --no-prune --size")
elif args[:2] == ["buildx", "du"]:
    assert args == ["buildx", "du", "--builder", "default", "--format", "json"]
    for name in state["caches"]:
        print(json.dumps({"ID": name, "Size": "123B", "Reclaimable": True, "Description": "fixture cache"}))
elif args[:2] == ["buildx", "prune"]:
    assert args[:7] == ["buildx", "prune", "--builder", "default", "--force", "--all", "--filter"]
    assert len(args) == 8 and args[7].startswith("id=^") and args[7].endswith("$")
    state["caches"].remove(args[7][4:-1])
    (root / "engine.json").write_text(json.dumps(state))
    print("Total: 123B")
elif args[:2] == ["image", "ls"]:
    for image in state["images"]:
        if "--filter" not in args or (args[-1] == "dangling=true") == image.endswith("a" * 64):
            print(image)
elif args[:2] == ["image", "inspect"]:
    print(json.dumps({"ID": args[-1], "Size": 2048, "Tags": [] if args[-1].endswith("a" * 64) else ["fixture:keep"]}))
elif args[:2] == ["container", "ls"]:
    if not any(arg.startswith("ancestor=") for arg in args):
        print("\n".join(state["stopped"]))
elif args[:2] == ["container", "inspect"]:
    print(json.dumps({"ID": args[-1], "Size": 512, "Name": "/fixture-stopped", "State": "exited"}))
elif args[:2] == ["image", "rm"]:
    assert args[:3] == ["image", "rm", "--no-prune"] and len(args) == 4
    state["images"].remove(args[3])
    (root / "engine.json").write_text(json.dumps(state))
    print(args[3])
elif args[:2] == ["container", "rm"]:
    assert len(args) == 3
    state["stopped"].remove(args[2])
    (root / "engine.json").write_text(json.dumps(state))
    print(args[2])
else:
    raise AssertionError(f"Forbidden command: {args!r}")
