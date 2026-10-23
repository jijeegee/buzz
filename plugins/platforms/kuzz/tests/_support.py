"""Shared helpers: put the plugin package on sys.path and drive the fake kuzz CLI."""

import json
import os
import sys
import tempfile
from pathlib import Path
from unittest import mock

TESTS = Path(__file__).resolve().parent
PLUGIN = TESTS.parent
sys.path.insert(0, str(PLUGIN.parent))  # makes the plugin importable as the `kuzz` package

OWNER = "a" * 64
STRANGER = "b" * 64
AGENT = "c" * 64
CHANNEL = "11111111-2222-3333-4444-555555555555"
DM = "99999999-8888-7777-6666-555555555555"


def event_id(n: int) -> str:
    return f"{n:064x}"


def watch_line(n: int, **overrides) -> dict:
    line = {
        "id": event_id(n),
        "channel": CHANNEL,
        "author": OWNER,
        "author_name": "Owner",
        "content": "hello",
        "created_at": 2_000_000_000,
        "thread_root": None,
        "mentions": [],
    }
    line.update(overrides)
    return line


class FakeKuzz:
    """Temp state dir + env for tests/fake_kuzz.py. Use as a context manager."""

    def __init__(self, **env):
        self._tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self._tmp.name)
        self.cli = [sys.executable, str(TESTS / "fake_kuzz.py")]
        # A single executable, for code that resolves the CLI path itself (KUZZ_CLI_PATH).
        if os.name == "nt":
            self.exe = self.dir / "kuzz.cmd"
            self.exe.write_text(f'@"{sys.executable}" "{TESTS / "fake_kuzz.py"}" %*\r\n', encoding="utf-8")
        else:
            self.exe = self.dir / "kuzz"
            self.exe.write_text(f'#!/bin/sh\nexec "{sys.executable}" "{TESTS / "fake_kuzz.py"}" "$@"\n', encoding="utf-8")
            self.exe.chmod(0o755)
        values = {"FAKE_KUZZ_DIR": str(self.dir)}
        for key, value in env.items():
            values[key] = value if isinstance(value, str) else json.dumps(value)
        self._patch = mock.patch.dict(os.environ, values)
        from kuzz import kuzz_cli

        # The fake CLI is a .cmd on Windows, which the plugin otherwise refuses to run.
        self._script_patch = mock.patch.object(kuzz_cli, "_ALLOW_SCRIPT_CLI", True)

    def __enter__(self):
        self._patch.start()
        self._script_patch.start()
        # A clean slate: none of the developer's own Buzz/Kuzz settings may leak into a test.
        for name in [k for k in os.environ if k.startswith(("BUZZ_", "KUZZ_")) and k not in self._patch.values]:
            del os.environ[name]
        return self

    def __exit__(self, *exc):
        self._script_patch.stop()
        self._patch.stop()
        self._tmp.cleanup()

    def calls(self, *prefix):
        path = self.dir / "calls.jsonl"
        if not path.exists():
            return []
        records = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()]
        return [r for r in records if r["args"][: len(prefix)] == list(prefix)]
