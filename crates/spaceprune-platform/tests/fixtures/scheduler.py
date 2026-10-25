"""Private command fixture. It only changes the supplied test directory."""
import os
from pathlib import Path
import sys

root = Path(os.environ["SPACEPRUNE_SCHEDULER_FIXTURE"])
state = root / "state"
state.mkdir(exist_ok=True)
scheduler = sys.argv[1]
args = sys.argv[2:]
verb = args[0].lower().lstrip("/")
count_path = state / (verb + ".count")
count = int(count_path.read_text()) + 1 if count_path.exists() else 1
count_path.write_text(str(count))
with (state / "trace").open("a") as trace:
    trace.write(" ".join(args) + "\n")
fault = state / f"{verb}.{count}.fail"
mode = fault.read_text() if fault.exists() else ""


def fail():
    print(f"fixture {verb} failure {count} ({mode})", file=sys.stderr)
    sys.exit(71)


if mode == "before":
    fail()

if scheduler == "launchctl":
    loaded = state / "loaded"
    disabled = state / "disabled"
    if verb == "print":
        if args[1].count("/") == 1:
            if (state / "unavailable-domain").exists():
                sys.exit(112)
            print("gui/501 = {}")
        elif loaded.exists():
            print("service loaded")
        else:
            sys.exit(113)
    elif verb == "print-disabled":
        if (state / "malformed-disabled").exists():
            print("unexpected launchctl output")
        else:
            print('disabled services = {\n    "io.spaceprune.daily" => '
                  + ("true" if disabled.exists() else "false") + '\n}')
    elif verb == "bootout":
        if not loaded.exists():
            sys.exit(113)
        loaded.unlink()
        if (state / "replace-definition-with-directory").exists():
            path = root / "home/Library/LaunchAgents/io.spaceprune.daily.plist"
            path.unlink()
            path.mkdir()
    elif verb == "bootstrap":
        if disabled.exists():
            print("service is disabled", file=sys.stderr)
            sys.exit(71)
        loaded.write_bytes(Path(args[2]).read_bytes())
    elif verb == "enable":
        disabled.unlink(missing_ok=True)
    elif verb == "disable":
        disabled.touch()
    else:
        raise AssertionError(args)
else:
    task = state / "task.xml"
    disabled = state / "disabled"
    if verb == "query":
        if not task.exists():
            # A POSIX process cannot report a Windows HRESULT. Exact HRESULT
            # absence classification is tested separately in Rust.
            sys.exit(2)
        sys.stdout.buffer.write(task.read_bytes())
    elif verb == "create":
        if "/XML" in args:
            xml = Path(args[args.index("/XML") + 1]).read_bytes()
        else:
            xml = b"<Task><Enabled>true</Enabled><Schedule>new</Schedule></Task>"
        task.write_bytes(xml)
        if b"<Enabled>false</Enabled>" in xml:
            disabled.touch()
        else:
            disabled.unlink(missing_ok=True)
    elif verb == "delete":
        if not task.exists():
            sys.exit(2)
        task.unlink()
        disabled.unlink(missing_ok=True)
    elif verb == "change":
        if not task.exists():
            sys.exit(2)
        assert "/DISABLE" in args
        disabled.touch()
    else:
        raise AssertionError(args)

if mode == "after":
    fail()
