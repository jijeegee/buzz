"""A fake `kuzz` CLI for the plugin tests. Driven by env vars; logs every call.

FAKE_KUZZ_DIR       state dir: calls.jsonl (one record per call) and the watch run counter
FAKE_KUZZ_WATCH     JSON list of watch runs: {"lines": [...], "stderr": "...", "rc": 0, "hang": false};
                    run N uses entry N (the last entry repeats)
FAKE_KUZZ_SEND_RC   exit code for `messages send` (default 0); FAKE_KUZZ_SEND_STDERR its stderr
FAKE_KUZZ_SELF      pubkey for `auth whoami` / `users get`; FAKE_KUZZ_NAME the display name
FAKE_KUZZ_OWNER     when set, the CLI is paired: keyless `auth whoami` reports this owner
                    ("null": paired, owner not known yet).
                    Otherwise whoami needs KUZZ_PRIVATE_KEY in the env (legacy) or fails with not_paired.
FAKE_KUZZ_DMS       JSON for `dms list` (default [])
FAKE_KUZZ_CHANNEL_NAME  name returned by `channels get`
"""

import json
import os
import sys
import time
from pathlib import Path

STATE = Path(os.environ["FAKE_KUZZ_DIR"])
WATCHED_ENV = (
    "KUZZ_ALLOW_ALL_USERS",
    "BUZZ_ALLOW_ALL_USERS",
    "KUZZ_PRIVATE_KEY",
    "BUZZ_PRIVATE_KEY",
    "KUZZ_AUTH_TAG",
    "BUZZ_AUTH_TAG",
    "KUZZ_RELAY_URL",
    "KUZZ_BOT_TOKEN",
    "KUZZ_ACCESS_TOKEN",
    "FAKE_KUZZ_MARKER",
)


def log(record):
    with open(STATE / "calls.jsonl", "a", encoding="utf-8", newline="\n") as fh:
        fh.write(json.dumps(record) + "\n")


def next_watch_run():
    counter = STATE / "watch_runs"
    n = int(counter.read_text()) if counter.exists() else 0
    counter.write_text(str(n + 1))
    runs = json.loads(os.environ.get("FAKE_KUZZ_WATCH", "[]")) or [{}]
    return runs[min(n, len(runs) - 1)]


def main():
    args = sys.argv[1:]
    stdin = sys.stdin.buffer.read().decode("utf-8") if "--content" in args and "-" in args else None
    log({"args": args, "stdin": stdin, "env": sorted(k for k in WATCHED_ENV if k in os.environ)})
    me = os.environ.get("FAKE_KUZZ_SELF", "")
    if args[:2] == ["messages", "watch"]:
        run = next_watch_run()
        for line in run.get("lines", []):
            sys.stdout.write((line if isinstance(line, str) else json.dumps(line)) + "\n")
            sys.stdout.flush()
        if run.get("hang"):
            while True:
                time.sleep(0.1)
        if run.get("stderr"):
            sys.stderr.write(run["stderr"] + "\n")
        return int(run.get("rc", 0))
    if args[:2] == ["messages", "send"]:
        rc = int(os.environ.get("FAKE_KUZZ_SEND_RC", "0"))
        if rc:
            sys.stderr.write(os.environ.get("FAKE_KUZZ_SEND_STDERR", "") + "\n")
            return rc
        count = sum(1 for _ in open(STATE / "calls.jsonl", encoding="utf-8"))
        print(json.dumps({"event_id": f"{count:064x}", "accepted": True, "message": ""}))
        return 0
    if args[:2] == ["auth", "whoami"]:
        owner = os.environ.get("FAKE_KUZZ_OWNER", "")
        if owner and "KUZZ_PRIVATE_KEY" not in os.environ:
            print(json.dumps({"mode": "paired", "pubkey": me, "agent_name": "Hermes", "device_name": "pc",
                              "relay": "https://relay.example", "owner": None if owner == "null" else owner}))
        elif "KUZZ_PRIVATE_KEY" in os.environ:
            print(json.dumps({"mode": "key", "pubkey": me}))
        else:
            sys.stderr.write(json.dumps({"error": "not_paired", "message": "not paired", "retryable": False}) + "\n")
            return 3
        return 0
    if args[:2] in (["messages", "edit"], ["messages", "delete"], ["reactions", "add"]):
        count = sum(1 for _ in open(STATE / "calls.jsonl", encoding="utf-8"))
        print(json.dumps({"event_id": f"{count:064x}", "accepted": True}))
        return 0
    if args[:2] == ["channels", "get"]:
        print(json.dumps({"id": args[-1], "name": os.environ.get("FAKE_KUZZ_CHANNEL_NAME", "general")}))
        return 0
    if args[:2] == ["users", "get"]:
        print(json.dumps([{"pubkey": me, "display_name": os.environ.get("FAKE_KUZZ_NAME", "")}]))
        return 0
    if args[:2] == ["dms", "list"]:
        print(os.environ.get("FAKE_KUZZ_DMS", "[]"))
        return 0
    sys.stderr.write(json.dumps({"error": "usage", "message": "unknown command"}) + "\n")
    return 1


if __name__ == "__main__":
    sys.exit(main())
