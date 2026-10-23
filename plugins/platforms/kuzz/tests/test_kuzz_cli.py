"""Transport tests: watch-line parsing, sending, child env, supervision, install.

Stdlib only; run with `python -m unittest discover -s plugins/platforms/kuzz/tests`.
"""

import asyncio
import json
import os
import tempfile
import time
import unittest
from pathlib import Path

from _support import AGENT, CHANNEL, OWNER, PLUGIN, FakeKuzz, event_id, watch_line

from kuzz import kuzz_cli
from kuzz.kuzz_cli import WatchSupervisor, parse_send_receipt, parse_watch_line

# A published npub/hex pair (NIP-19 test vector style).
NPUB = "npub180cvv07tjdrrgpa0j7j7tmnyl2yr6yr7l8j4s3evf6u64th6gkwsyjh6w6"
NPUB_HEX = "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d"


def run(coro):
    return asyncio.run(coro)


class ParseWatchLineTest(unittest.TestCase):
    def test_full_line(self):
        line = watch_line(1, thread_root=event_id(9), mentions=[AGENT, "not-a-key"], extra_field=True)
        msg = parse_watch_line(json.dumps(line).encode())
        self.assertEqual(msg.id, event_id(1))
        self.assertEqual(msg.channel, CHANNEL)
        self.assertEqual(msg.author, OWNER)
        self.assertEqual(msg.author_name, "Owner")
        self.assertEqual(msg.content, "hello")
        self.assertEqual(msg.created_at, 2_000_000_000)
        self.assertEqual(msg.thread_root, event_id(9))
        self.assertEqual(msg.mentions, (AGENT,))

    def test_npub_author_and_mentions_become_hex(self):
        msg = parse_watch_line(json.dumps(watch_line(1, author=NPUB.upper(), mentions=[NPUB])))
        self.assertEqual(msg.author, NPUB_HEX)
        self.assertEqual(msg.mentions, (NPUB_HEX,))

    def test_optional_fields(self):
        line = watch_line(1, thread_root="", author_name="  ")
        del line["mentions"]
        msg = parse_watch_line(json.dumps(line))
        self.assertIsNone(msg.thread_root)
        self.assertIsNone(msg.author_name)
        self.assertEqual(msg.mentions, ())

    def test_rejects_non_messages(self):
        bad = [
            "",
            "   ",
            "not json",
            "[1, 2]",
            json.dumps(watch_line(1, author="someone")),
            json.dumps(watch_line(1, content=None)),
            json.dumps(watch_line(1, created_at="soon")),
            json.dumps(watch_line(1, created_at=True)),
            json.dumps(watch_line(1, channel="")),
            json.dumps({k: v for k, v in watch_line(1).items() if k != "id"}),
        ]
        for line in bad:
            with self.subTest(line=line):
                self.assertIsNone(parse_watch_line(line))

    def test_bad_npub_checksum(self):
        self.assertIsNone(kuzz_cli.normalize_pubkey(NPUB[:-1] + "q"))


class EnvAndRedactionTest(unittest.TestCase):
    def test_child_env_strips_allow_all_and_credentials(self):
        base = {name: "x" for name in kuzz_cli.STRIPPED_ENV}
        base.update(PATH="/bin", KUZZ_RELAY_URL="wss://relay", KUZZ_DEVICE_KEY_REF="ref")
        home = Path(tempfile.gettempdir()) / "hermes-home-test"
        env = kuzz_cli.child_env(base, home=home)
        session_file = str(home / "buzz" / "kuzz-no-login" / "session.json")
        self.assertEqual(
            env,
            {
                "PATH": "/bin",
                "KUZZ_RELAY_URL": "wss://relay",
                "KUZZ_DEVICE_KEY_REF": "ref",
                # A person's `kuzz auth login` must never outrank the paired agent.
                "KUZZ_SESSION_FILE": session_file,
            },
        )
        self.assertFalse(Path(session_file).exists())
        for name in ("KUZZ_ALLOW_ALL_USERS", "BUZZ_ALLOW_ALL_USERS", "KUZZ_TOKEN_BROKER_SECRET",
                     "KUZZ_DEVICE_BROKER_URL", "KUZZ_DEVICE_BROKER_SECRET", "KUZZ_DISABLE_STORED_SESSION"):
            self.assertIn(name, kuzz_cli.STRIPPED_ENV)

    def test_redact(self):
        text = "bad nsec1qqqqqqqqqqqqqqqq Authorization: Bearer abc.def run kuzz pair K7Q2-SECRET " + "x" * 900
        out = kuzz_cli.redact(text)
        for secret in ("nsec1qqqqqqqqqqqqqqqq", "abc.def", "K7Q2-SECRET"):
            self.assertNotIn(secret, out)
        self.assertLessEqual(len(out), 500)

    def test_redact_pair_command_and_hex(self):
        code = "ABCdef0123456789_-xyzQ"
        out = kuzz_cli.redact(f"run: kuzz pair --relay https://r.example {code}\nnext")
        self.assertNotIn(code, out)
        self.assertIn("--relay https://r.example [redacted]", out)
        out = kuzz_cli.redact(f"kuzz.exe pair {code} --relay=wss://r --hermes")
        self.assertNotIn(code, out)
        out = kuzz_cli.redact("key " + "ab" * 32 + " end")
        self.assertEqual(out, "key [hex64] end")

    def test_classify_exit_parks_on_auth_words_only_with_exit_3(self):
        self.assertEqual(kuzz_cli.classify_exit(3, '{"error":"auth","message":"revoked"}')[0], "revoked")
        self.assertIsNone(kuzz_cli.classify_exit(2, "network: peer not_paired? revoked"))
        self.assertIsNone(kuzz_cli.classify_exit(3, "unrevoked"))
        self.assertEqual(kuzz_cli.classify_exit(2, "error: unrecognized subcommand 'watch'")[0],
                         "unrecognized_subcommand")


class SendTest(unittest.TestCase):
    def test_send_in_thread_through_stdin(self):
        text = "line one\nline two \u00e9\u4e2d\U0001f41d --channel x"
        with FakeKuzz(KUZZ_ALLOW_ALL_USERS="true", BUZZ_ALLOW_ALL_USERS="true", KUZZ_PRIVATE_KEY="secret",
                      FAKE_KUZZ_MARKER="kept") as fake:
            outcome = run(kuzz_cli.send_message(fake.cli, CHANNEL, text, thread=event_id(7)))
            [call] = fake.calls("messages", "send")
        self.assertIsNone(outcome.error)
        self.assertRegex(outcome.event_id, r"^[0-9a-f]{64}$")
        self.assertEqual(
            call["args"],
            ["messages", "send", "--channel", CHANNEL, "--reply-in-thread", event_id(7), "--content", "-"],
        )
        self.assertEqual(call["stdin"], text)
        self.assertEqual(call["env"], ["FAKE_KUZZ_MARKER"])

    def test_send_top_level(self):
        with FakeKuzz() as fake:
            run(kuzz_cli.send_message(fake.cli, CHANNEL, "hi"))
            [call] = fake.calls("messages", "send")
        self.assertEqual(call["args"], ["messages", "send", "--channel", CHANNEL, "--content", "-"])

    def test_send_failure_is_bounded_and_classified(self):
        stderr = json.dumps({"error": "network", "message": "relay unreachable", "retryable": True})
        with FakeKuzz(FAKE_KUZZ_SEND_RC="2", FAKE_KUZZ_SEND_STDERR=stderr) as fake:
            outcome = run(kuzz_cli.send_message(fake.cli, CHANNEL, "hi"))
        self.assertIsNone(outcome.event_id)
        self.assertEqual(outcome.error, "network: relay unreachable (exit 2)")
        self.assertTrue(outcome.retryable)
        # Exit 2 may mean "delivery unknown": without the CLI's retryable flag, never resend.
        unknown = json.dumps({"error": "network", "message": "delivery unknown", "retryable": False})
        for err in (unknown, "network: reset"):
            with FakeKuzz(FAKE_KUZZ_SEND_RC="2", FAKE_KUZZ_SEND_STDERR=err) as fake:
                self.assertFalse(run(kuzz_cli.send_message(fake.cli, CHANNEL, "hi")).retryable)
        with FakeKuzz(FAKE_KUZZ_SEND_RC="1", FAKE_KUZZ_SEND_STDERR="usage") as fake:
            self.assertFalse(run(kuzz_cli.send_message(fake.cli, CHANNEL, "hi")).retryable)

    def test_receipt(self):
        self.assertEqual(parse_send_receipt('{"event_id": "e1", "accepted": true}'), ("e1", None))
        self.assertEqual(parse_send_receipt('{"accepted": false, "message": "blocked"}'), (None, "blocked"))
        self.assertEqual(parse_send_receipt('{"event_id": "", "accepted": true}'), (None, "invalid kuzz response"))
        self.assertEqual(parse_send_receipt("oops"), (None, "invalid kuzz response"))


class SupervisorTest(unittest.TestCase):
    def _supervise(self, fake, *, timeout=20.0, stop_after=None):
        received, parked = [], []

        async def on_message(msg):
            received.append(msg)

        async def on_park(code, message):
            parked.append((code, message))

        async def main():
            sup = WatchSupervisor(fake.cli, on_message, on_park=on_park, min_backoff=0.05, max_backoff=0.2)
            task = asyncio.ensure_future(sup.run())
            if stop_after is not None:
                await stop_after(received)
                started = time.monotonic()
                await sup.stop()
                await asyncio.wait_for(task, timeout)
                return sup, time.monotonic() - started
            await asyncio.wait_for(task, timeout)
            return sup, 0.0

        sup, stop_secs = run(main())
        return sup, received, parked, stop_secs

    def test_restarts_with_backoff_then_parks_when_signed_out(self):
        runs = [
            {"lines": [watch_line(1), "garbage", watch_line(1, id="")], "rc": 2, "stderr": "network: reset"},
            {"lines": [watch_line(2)], "rc": 0},
            {"lines": [], "rc": 3, "stderr": json.dumps({"error": "auth", "message": "signed_out"})},
        ]
        with FakeKuzz(FAKE_KUZZ_WATCH=runs, KUZZ_ALLOW_ALL_USERS="true") as fake:
            sup, received, parked, _ = self._supervise(fake)
            watch_calls = fake.calls("messages", "watch")
        self.assertEqual([m.id for m in received], [event_id(1), event_id(2)])
        self.assertEqual(sup.restarts, 2)
        self.assertEqual(len(watch_calls), 3)
        self.assertTrue(all(call["env"] == [] for call in watch_calls))
        self.assertEqual(parked[0][0], "signed_out")
        self.assertEqual(sup.parked[0], "signed_out")

    def test_old_cli_without_watch_parks(self):
        runs = [{"rc": 2, "stderr": "error: unrecognized subcommand 'watch'"}]
        with FakeKuzz(FAKE_KUZZ_WATCH=runs) as fake:
            sup, _, parked, _ = self._supervise(fake)
        self.assertEqual(parked[0][0], "unrecognized_subcommand")
        self.assertEqual(sup.restarts, 0)

    def test_stop_ends_a_running_watch(self):
        async def first_message(received):
            while not received:
                await asyncio.sleep(0.02)

        with FakeKuzz(FAKE_KUZZ_WATCH=[{"lines": [watch_line(1)], "hang": True}]) as fake:
            sup, received, parked, stop_secs = self._supervise(fake, stop_after=first_message)
        self.assertEqual(len(received), 1)
        self.assertEqual(parked, [])
        self.assertEqual(sup.restarts, 0)
        self.assertLess(stop_secs, 10)


class CliPathTest(unittest.TestCase):
    def test_configured_path_decides_alone(self):
        with FakeKuzz() as fake:
            self.assertEqual(kuzz_cli.resolve_cli_path("", str(fake.exe), "other"), str(fake.exe))
            self.assertEqual(kuzz_cli.resolve_cli_path(str(fake.dir / "missing.exe"), str(fake.exe)), "")

    @unittest.skipUnless(os.name == "nt", "Windows only")
    def test_windows_runs_only_exe(self):
        with tempfile.TemporaryDirectory() as tmp:
            for name in ("kuzz.cmd", "kuzz.bat", "kuzz.exe"):
                (Path(tmp) / name).write_bytes(b"")
            self.assertEqual(kuzz_cli.resolve_cli_path(str(Path(tmp) / "kuzz.cmd")), "")
            self.assertFalse(kuzz_cli.is_runnable(str(Path(tmp) / "kuzz.bat")))
            self.assertTrue(kuzz_cli.is_runnable(str(Path(tmp) / "kuzz.exe")))

    def test_send_with_files_names_only_in_errors(self):
        stderr = json.dumps({"error": "usage", "message": "cannot read C:/secret/dir/chart.png"})
        with FakeKuzz(FAKE_KUZZ_SEND_RC="1", FAKE_KUZZ_SEND_STDERR=stderr) as fake:
            outcome = run(kuzz_cli.send_message(fake.cli, CHANNEL, "", files=["C:/secret/dir/chart.png"]))
            [call] = fake.calls("messages", "send")
        self.assertEqual(call["args"], ["messages", "send", "--channel", CHANNEL, "--file", "C:/secret/dir/chart.png", "--content", "-"])
        self.assertNotIn("C:/secret", outcome.error)


class InstallTest(unittest.TestCase):
    def test_install_replaces_the_bundled_buzz_plugin(self):
        import importlib.util

        spec = importlib.util.spec_from_file_location("kuzz_install", PLUGIN / "install.py")
        install = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(install)
        manifest = (PLUGIN / "plugin.yaml").read_text(encoding="utf-8")
        self.assertIn("name: buzz-platform\n", manifest)  # the bundled plugin's key: ours replaces it
        self.assertEqual(install.PLUGIN_NAME, "buzz-platform")
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            (home / ".env").write_bytes(
                b"OTHER=1\r\nKUZZ_OWNER_PUBKEY=old\r\nBUZZ_CLI_PATH=C:/old/buzz.exe\r\nexport KUZZ_OWNER_PUBKEY=older\r\n")
            (home / "plugins" / "buzz").mkdir(parents=True)
            (home / "plugins" / "buzz" / "stale.py").write_text("", encoding="utf-8")
            rc = install.main(["--owner", NPUB, "--hermes-home", str(home), "--cli-path", "C:/k/kuzz.exe", "--no-enable"])
            self.assertEqual(rc, 0)
            files = sorted(p.name for p in (home / "plugins" / "buzz").iterdir())
            self.assertEqual(files, sorted(install.PLUGIN_FILES))
            # The old folder is kept, outside plugins/ so Hermes does not load it twice.
            [backup] = (home / "plugin-backups").iterdir()
            self.assertTrue(backup.name.startswith("buzz-"))
            self.assertTrue((backup / "stale.py").is_file())
            self.assertEqual(sorted(p.name for p in (home / "plugins").iterdir()), ["buzz"])
            # Every copy of a key is updated; CRLF stays CRLF.
            self.assertEqual(
                (home / ".env").read_bytes().decode("utf-8"),
                f"OTHER=1\r\nKUZZ_OWNER_PUBKEY={NPUB_HEX}\r\nBUZZ_CLI_PATH=C:/old/buzz.exe\r\n"
                f"export KUZZ_OWNER_PUBKEY={NPUB_HEX}\r\nKUZZ_CLI_PATH=C:/k/kuzz.exe\r\n",
            )
            with self.assertRaises(SystemExit):
                install.main(["--owner", "not-a-key", "--hermes-home", str(home), "--no-enable"])


if __name__ == "__main__":
    unittest.main()
