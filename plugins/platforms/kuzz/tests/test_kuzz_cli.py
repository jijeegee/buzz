"""Transport tests: watch-line parsing, sending, child env, supervision, install.

Stdlib only; run with `python -m unittest discover -s plugins/platforms/kuzz/tests`.
"""

import asyncio
import json
import os
import shutil
import subprocess
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

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
        session_file = str(home / "kuzz" / "no-login" / "session.json")
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


class UpdatePauseTest(unittest.TestCase):
    """The installer's signal file: ``%LOCALAPPDATA%\\kuzz\\update-pause``."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        for patch in (
            mock.patch.dict(os.environ, {"LOCALAPPDATA": tmp.name}),
            mock.patch.object(kuzz_cli, "UPDATE_PAUSE_POLL", 0.02),
        ):
            patch.start()
            self.addCleanup(patch.stop)
        self.signal = Path(tmp.name) / "kuzz" / "update-pause"

    def _pause(self, age=0.0):
        self.signal.parent.mkdir(exist_ok=True)
        self.signal.touch()
        stamp = time.time() - age
        os.utime(self.signal, (stamp, stamp))

    def _supervise(self, fake, script):
        """Run a supervisor beside ``script(received, sup)``, then stop it."""
        received = []

        async def on_message(msg):
            received.append(msg)

        async def main():
            sup = WatchSupervisor(fake.cli, on_message, min_backoff=30, pause_poll=0.02)
            task = asyncio.ensure_future(sup.run())
            await asyncio.wait_for(script(received, sup), 20)
            await sup.stop()
            await asyncio.wait_for(task, 20)
            return sup

        return run(main()), received

    @staticmethod
    async def _until(condition):
        while not condition():
            await asyncio.sleep(0.02)

    def test_signal_counts_only_while_fresh(self):
        self.assertFalse(kuzz_cli.update_paused())
        self._pause()
        self.assertTrue(kuzz_cli.update_paused())
        self._pause(age=kuzz_cli.UPDATE_PAUSE_MAX_AGE + 1)
        self.assertFalse(kuzz_cli.update_paused())

    def test_no_local_data_means_no_pause(self):
        with mock.patch.dict(os.environ):
            del os.environ["LOCALAPPDATA"]
            self.assertIsNone(kuzz_cli.update_pause_file())
            self.assertFalse(kuzz_cli.update_paused())

    def test_pause_ends_the_watch_and_resume_skips_the_backoff(self):
        runs = [{"lines": [watch_line(1)], "hang": True}, {"lines": [watch_line(2)], "hang": True}]
        with FakeKuzz(FAKE_KUZZ_WATCH=runs) as fake:

            async def script(received, sup):
                await self._until(lambda: received)
                self._pause()
                await self._until(lambda: sup._ended_for_update and sup._proc is None)
                await asyncio.sleep(0.1)
                self.assertEqual(len(fake.calls("messages", "watch")), 1)  # held, not restarted
                self.signal.unlink()
                await self._until(lambda: len(received) == 2)

            sup, received = self._supervise(fake, script)
        self.assertEqual([m.id for m in received], [event_id(1), event_id(2)])
        self.assertEqual(sup.restarts, 0)
        self.assertIsNone(sup.parked)

    def test_watch_does_not_start_during_a_pause(self):
        self._pause()
        with FakeKuzz(FAKE_KUZZ_WATCH=[{"lines": [watch_line(1)], "hang": True}]) as fake:

            async def script(received, sup):
                await asyncio.sleep(0.2)
                self.assertEqual(fake.calls("messages", "watch"), [])
                self.signal.unlink()
                await self._until(lambda: received)

            sup, _ = self._supervise(fake, script)
        self.assertEqual(sup.restarts, 0)

    def test_stop_ends_a_held_watch(self):
        self._pause()
        with FakeKuzz() as fake:

            async def script(received, sup):
                await asyncio.sleep(0.05)

            self._supervise(fake, script)
            self.assertEqual(fake.calls("messages", "watch"), [])

    def test_stale_signal_does_not_stop_the_watch(self):
        self._pause(age=kuzz_cli.UPDATE_PAUSE_MAX_AGE + 1)
        with FakeKuzz(FAKE_KUZZ_WATCH=[{"lines": [watch_line(1)], "hang": True}]) as fake:

            async def script(received, sup):
                await self._until(lambda: received)
                await asyncio.sleep(0.1)

            sup, _ = self._supervise(fake, script)
            self.assertEqual(len(fake.calls("messages", "watch")), 1)
        self.assertFalse(sup._ended_for_update)

    def test_send_waits_for_the_pause_to_end(self):
        self._pause()
        with FakeKuzz() as fake:

            async def main():
                send = asyncio.ensure_future(kuzz_cli.send_message(fake.cli, CHANNEL, "later"))
                await asyncio.sleep(0.1)
                self.assertFalse(send.done())
                self.assertEqual(fake.calls("messages", "send"), [])
                self.signal.unlink()
                return await asyncio.wait_for(send, 20)

            outcome = run(main())
            self.assertIsNotNone(outcome.event_id)
            self.assertEqual(fake.calls("messages", "send")[0]["stdin"], "later")

    def test_send_retries_when_the_file_is_replaced_under_a_pause(self):
        real = asyncio.create_subprocess_exec
        attempts = []

        async def spawn(*args, **kwargs):
            attempts.append(args)
            if len(attempts) == 1:
                self._pause()
                raise PermissionError("kuzz.exe is being replaced")
            return await real(*args, **kwargs)

        with FakeKuzz() as fake:

            async def main():
                with mock.patch.object(kuzz_cli.asyncio, "create_subprocess_exec", spawn):
                    send = asyncio.ensure_future(kuzz_cli.send_message(fake.cli, CHANNEL, "later"))
                    await self._until(lambda: attempts)
                    await asyncio.sleep(0.1)
                    self.assertEqual(len(attempts), 1)
                    self.signal.unlink()
                    return await asyncio.wait_for(send, 20)

            outcome = run(main())
        self.assertIsNotNone(outcome.event_id)
        self.assertEqual(len(attempts), 2)

    def test_a_spawn_error_without_a_pause_is_reported(self):
        outcome = run(kuzz_cli.send_message([str(self.signal.parent / "missing.exe")], CHANNEL, "x"))
        self.assertIsNone(outcome.event_id)
        self.assertIn("could not start kuzz", outcome.error)

    def test_hold_gives_up_at_its_limit(self):
        self._pause()
        self.assertFalse(run(kuzz_cli.hold_for_update(limit=0.05)))
        self.signal.unlink()
        self.assertTrue(run(kuzz_cli.hold_for_update(limit=0.05)))


class CliPathTest(unittest.TestCase):
    def test_configured_path_decides_alone(self):
        with FakeKuzz() as fake:
            self.assertEqual(kuzz_cli.resolve_cli_path("", str(fake.exe), "other"), str(fake.exe))
            buzz = fake.dir / ("buzz.exe" if os.name == "nt" else "buzz")
            shutil.copy2(fake.exe, buzz)
            with mock.patch.dict(os.environ, {"PATH": str(fake.dir)}), \
                    mock.patch.object(kuzz_cli, "find_kuzz", return_value=""):
                self.assertEqual(kuzz_cli.resolve_cli_path(), "")
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
    def _install(self):
        import importlib.util

        spec = importlib.util.spec_from_file_location("kuzz_install", PLUGIN / "install.py")
        install = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(install)
        return install

    def _old_folder(self, home, version):
        folder = home / "plugins" / "buzz"
        folder.mkdir(parents=True)
        (folder / "plugin.yaml").write_text(f"name: buzz-platform\nversion: {version}\n", encoding="utf-8")
        return folder

    def test_an_earlier_kuzz_build_gives_buzz_its_slot_back(self):
        install = self._install()
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            folder = self._old_folder(home, "2.0.0-kuzz")
            rc = install.main(["--hermes-home", str(home), "--cli-path", "C:/k/kuzz.exe", "--no-enable"])
            self.assertEqual(rc, 0)
            self.assertFalse(folder.exists())
            self.assertEqual([p.name for p in (home / "plugins").iterdir()], ["kuzz"])
            [backup] = (home / "plugin-backups").iterdir()
            self.assertTrue(backup.name.startswith("buzz-"))

    def test_a_real_buzz_plugin_is_left_alone(self):
        install = self._install()
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            folder = self._old_folder(home, "1.0.0")
            install.main(["--hermes-home", str(home), "--cli-path", "C:/k/kuzz.exe", "--no-enable"])
            self.assertTrue((folder / "plugin.yaml").is_file())
            self.assertFalse((home / "plugin-backups").exists())
            self.assertEqual(sorted(p.name for p in (home / "plugins").iterdir()), ["buzz", "kuzz"])

    def test_enabling_swaps_the_old_name_only_after_retiring_it(self):
        install = self._install()
        writes = []

        def config(_hermes, _env, action, _key, value=None):
            if action == "set":
                writes.append(json.loads(value))
                return subprocess.CompletedProcess([], 0, "", "")
            return subprocess.CompletedProcess([], 0, "", "")

        for retired, enabled, expected in (
            (True, ["buzz-platform", "x"], ["x", "kuzz-platform"]),
            (False, ["buzz-platform"], ["buzz-platform", "kuzz-platform"]),
        ):
            with mock.patch.object(install, "find_hermes", return_value="hermes"), \
                    mock.patch.object(install, "_hermes_config", side_effect=config), \
                    mock.patch.object(install, "_read_list", side_effect=[enabled, []]):
                self.assertTrue(install.enable_plugin(Path("home"), retired))
            self.assertEqual(writes[-1], expected)

    def test_install_copies_the_plugin(self):
        install = self._install()
        manifest = (PLUGIN / "plugin.yaml").read_text(encoding="utf-8")
        self.assertIn("name: kuzz-platform\n", manifest)
        self.assertEqual(install.PLUGIN_NAME, "kuzz-platform")
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            (home / ".env").write_bytes(
                b"OTHER=1\r\nKUZZ_OWNER_PUBKEY=old\r\nBUZZ_CLI_PATH=C:/old/buzz.exe\r\nexport KUZZ_OWNER_PUBKEY=older\r\n")
            (home / "plugins" / "kuzz").mkdir(parents=True)
            (home / "plugins" / "kuzz" / "stale.py").write_text("", encoding="utf-8")
            rc = install.main(["--owner", NPUB, "--hermes-home", str(home), "--cli-path", "C:/k/kuzz.exe", "--no-enable"])
            self.assertEqual(rc, 0)
            files = sorted(p.name for p in (home / "plugins" / "kuzz").iterdir())
            self.assertEqual(files, sorted(install.PLUGIN_FILES))
            # The old folder is kept, outside plugins/ so Hermes does not load it twice.
            [backup] = (home / "plugin-backups").iterdir()
            self.assertTrue(backup.name.startswith("kuzz-"))
            self.assertTrue((backup / "stale.py").is_file())
            self.assertEqual(sorted(p.name for p in (home / "plugins").iterdir()), ["kuzz"])
            # Every copy of a key is updated; CRLF stays CRLF.
            self.assertEqual(
                (home / ".env").read_bytes().decode("utf-8"),
                f"OTHER=1\r\nKUZZ_OWNER_PUBKEY={NPUB_HEX}\r\nBUZZ_CLI_PATH=C:/old/buzz.exe\r\n"
                f"export KUZZ_OWNER_PUBKEY={NPUB_HEX}\r\nKUZZ_CLI_PATH=C:/k/kuzz.exe\r\n",
            )
            with self.assertRaises(SystemExit):
                install.main(["--owner", "not-a-key", "--hermes-home", str(home), "--no-enable"])


def load_install():
    import importlib.util

    spec = importlib.util.spec_from_file_location("kuzz_install", PLUGIN / "install.py")
    install = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(install)
    return install


class InstallHomeAndRestartTest(unittest.TestCase):
    def setUp(self):
        self.install = load_install()
        # A Hermes whose config.yaml has no Kuzz home yet; nothing real is run.
        self.config = subprocess.CompletedProcess([], 1, "", "Config key not set: platforms.kuzz.home_channel")
        for name, kwargs in (("find_hermes", {"return_value": "hermes"}),
                             ("_hermes_config", {"side_effect": lambda *a: self.config})):
            patcher = mock.patch.object(self.install, name, **kwargs)
            patcher.start()
            self.addCleanup(patcher.stop)

    def test_home_is_the_owner_dm_opened_by_the_paired_agent(self):
        with tempfile.TemporaryDirectory() as tmp, FakeKuzz(FAKE_KUZZ_DM_ID="dm-owner") as fake:
            home = Path(tmp)
            (home / ".env").write_text("OTHER=1\n", encoding="utf-8")
            agent = "a" * 64
            self.assertEqual(self.install.set_home_channel(home, str(fake.exe), NPUB_HEX, agent), "dm-owner")
            [call] = fake.calls("dms", "open")
            self.assertEqual(call["args"], ["dms", "open", "--pubkey", NPUB_HEX])
            self.assertNotIn("KUZZ_PRIVATE_KEY", call["env"])
            self.assertIn("KUZZ_PAIRED_AGENT", call["env"])
            self.assertEqual((home / ".env").read_text(encoding="utf-8"), "OTHER=1\nKUZZ_HOME_CHANNEL=dm-owner\n")

    def test_an_existing_home_is_kept(self):
        with tempfile.TemporaryDirectory() as tmp, FakeKuzz() as fake:
            home = Path(tmp)
            (home / ".env").write_text("KUZZ_HOME_CHANNEL='chan-1'\n", encoding="utf-8")
            self.assertIsNone(self.install.set_home_channel(home, str(fake.exe), NPUB_HEX))
            self.config = subprocess.CompletedProcess([], 0, "chan-2\n", "")
            (home / ".env").unlink()
            self.assertIsNone(self.install.set_home_channel(home, str(fake.exe), NPUB_HEX))
            self.assertEqual(fake.calls("dms", "open"), [])
            self.assertFalse((home / ".env").exists())

    def test_an_unreadable_config_or_no_hermes_leaves_the_home_alone(self):
        with tempfile.TemporaryDirectory() as tmp, FakeKuzz() as fake:
            home = Path(tmp)
            self.config = subprocess.CompletedProcess([], 1, "", "timed out")
            self.assertIsNone(self.install.set_home_channel(home, str(fake.exe), NPUB_HEX))
            with mock.patch.object(self.install, "find_hermes", return_value=None):
                self.assertIsNone(self.install.set_home_channel(home, str(fake.exe), NPUB_HEX))
            self.assertEqual(fake.calls("dms", "open"), [])
            self.assertFalse((home / ".env").exists())

    def test_a_failed_dm_open_leaves_no_home(self):
        with tempfile.TemporaryDirectory() as tmp, FakeKuzz(FAKE_KUZZ_DM_ID="") as fake:
            home = Path(tmp)
            self.assertIsNone(self.install.set_home_channel(home, str(fake.exe), NPUB_HEX))
            self.assertFalse((home / ".env").exists())

    def test_main_pins_the_agent_sets_home_then_restarts_the_gateway(self):
        with tempfile.TemporaryDirectory() as tmp, FakeKuzz() as fake,                 mock.patch.object(self.install, "enable_plugin", return_value=True),                 mock.patch.object(self.install, "restart_gateway", return_value=True) as restart:
            home = Path(tmp)
            agent = "b" * 64
            argv = ["--owner", NPUB, "--agent", agent, "--hermes-home", str(home), "--cli-path", str(fake.exe)]
            self.assertEqual(self.install.main(argv), 0)
            restart.assert_called_once_with(home)
            env = (home / ".env").read_text(encoding="utf-8")
            self.assertIn(f"KUZZ_PAIRED_AGENT={agent}", env)
            self.assertIn("KUZZ_HOME_CHANNEL=dm-1", env)
            with self.assertRaises(SystemExit):
                self.install.main(argv[:2] + ["--agent", "nope"] + argv[4:])

    def test_a_new_agent_replaces_the_old_agents_home(self):
        with tempfile.TemporaryDirectory() as tmp, FakeKuzz(FAKE_KUZZ_DM_ID="dm-new") as fake, \
                mock.patch.object(self.install, "enable_plugin", return_value=True), \
                mock.patch.object(self.install, "restart_gateway", return_value=True):
            home = Path(tmp)
            base = ["--owner", NPUB, "--hermes-home", str(home), "--cli-path", str(fake.exe), "--no-restart"]
            (home / ".env").write_text(f"KUZZ_PAIRED_AGENT={'a' * 64}\nKUZZ_HOME_CHANNEL=dm-old\n", encoding="utf-8")
            # Same agent again: the home stays.
            self.assertEqual(self.install.main(base + ["--agent", "a" * 64]), 0)
            self.assertIn("KUZZ_HOME_CHANNEL=dm-old", (home / ".env").read_text(encoding="utf-8"))
            # A new agent (re-connect): its own DM with the owner becomes the home.
            self.assertEqual(self.install.main(base + ["--agent", "b" * 64]), 0)
            env = (home / ".env").read_text(encoding="utf-8")
            self.assertIn("KUZZ_HOME_CHANNEL=dm-new", env)
            self.assertNotIn("dm-old", env)

    def test_a_named_profile_uses_its_own_env_and_the_host_gateway(self):
        with tempfile.TemporaryDirectory() as tmp, FakeKuzz() as fake, \
                mock.patch.object(self.install, "enable_plugin", return_value=True), \
                mock.patch.object(self.install, "find_hermes", return_value="hermes"), \
                mock.patch.object(self.install.subprocess, "run", wraps=subprocess.run) as run:
            root = Path(tmp)
            profile = root / "profiles" / "work"
            profile.mkdir(parents=True)
            (root / ".env").write_text(f"KUZZ_OWNER_PUBKEY={NPUB_HEX}\nKUZZ_HOME_CHANNEL=root-home\n", encoding="utf-8")
            (root / "plugins" / "kuzz").mkdir(parents=True)
            (root / "plugins" / "kuzz" / "old.txt").write_text("old", encoding="utf-8")
            agent, default_agent = "b" * 64, "a" * 64
            with mock.patch.object(self.install, "restart_gateway", return_value=True) as restart:
                self.assertEqual(self.install.main([
                    "--owner", NPUB, "--agent", agent, "--default-agent", f"{NPUB_HEX}={default_agent}",
                    "--default-agent", f"{'c' * 64}={'e' * 64}",
                    "--hermes-home", str(profile), "--cli-path", str(fake.exe)]), 0)
            restart.assert_called_once_with(profile)
            own = (profile / ".env").read_text(encoding="utf-8")
            self.assertIn(f"KUZZ_PAIRED_AGENT={agent}", own)
            self.assertIn("KUZZ_HOME_CHANNEL=dm-1", own)
            self.assertTrue((profile / "plugins" / "kuzz" / "plugin.yaml").exists())
            # The default profile's copy is refreshed with it.
            self.assertTrue((root / "plugins" / "kuzz" / "plugin.yaml").exists())
            self.assertFalse((root / "plugins" / "kuzz" / "old.txt").exists())
            # The default profile keeps its agent and its home.
            default = (root / ".env").read_text(encoding="utf-8")
            self.assertIn(f"KUZZ_PAIRED_AGENT={default_agent}", default)
            self.assertIn("KUZZ_HOME_CHANNEL=root-home", default)
            # The restart is the host's.
            run.side_effect = lambda *a, **k: subprocess.CompletedProcess([], 0, "", "")
            self.assertTrue(self.install.restart_gateway(profile))
            self.assertEqual(run.call_args.kwargs["env"]["HERMES_HOME"], str(root))

    def test_a_cloned_profiles_home_is_replaced(self):
        with tempfile.TemporaryDirectory() as tmp, FakeKuzz() as fake, \
                mock.patch.object(self.install, "enable_plugin", return_value=True):
            profile = Path(tmp) / "profiles" / "work"
            profile.mkdir(parents=True)
            (profile / ".env").write_text(f"KUZZ_OWNER_PUBKEY={NPUB_HEX}\nKUZZ_HOME_CHANNEL=sources-home\n", encoding="utf-8")
            self.assertEqual(self.install.main([
                "--owner", NPUB, "--agent", "b" * 64, "--hermes-home", str(profile),
                "--cli-path", str(fake.exe), "--no-restart"]), 0)
            self.assertIn("KUZZ_HOME_CHANNEL=dm-1", (profile / ".env").read_text(encoding="utf-8"))

    def test_nothing_is_pinned_when_the_plugin_cant_be_enabled(self):
        with tempfile.TemporaryDirectory() as tmp, FakeKuzz() as fake, \
                mock.patch.object(self.install, "enable_plugin", return_value=False):
            home = Path(tmp)
            self.assertEqual(self.install.main([
                "--owner", NPUB, "--agent", "b" * 64, "--hermes-home", str(home),
                "--cli-path", str(fake.exe), "--no-restart"]), 1)
            self.assertFalse((home / ".env").exists())

    def test_the_default_pin_needs_its_owners_agent_and_no_pin(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            profile = root / "profiles" / "work"
            profile.mkdir(parents=True)
            for text in (f"KUZZ_OWNER_PUBKEY={'c' * 64}\n", f"KUZZ_OWNER_PUBKEY={NPUB_HEX}\nKUZZ_PAIRED_AGENT={'d' * 64}\n"):
                (root / ".env").write_text(text, encoding="utf-8")
                self.install.pin_default_profile(profile, {NPUB_HEX: "a" * 64})
                self.assertEqual((root / ".env").read_text(encoding="utf-8"), text)
            # Another account's Connect keeps this account's agent here.
            (root / ".env").write_text(f"KUZZ_OWNER_PUBKEY={'c' * 64}\n", encoding="utf-8")
            self.install.pin_default_profile(profile, {NPUB_HEX: "a" * 64, "c" * 64: "e" * 64})
            self.assertIn(f"KUZZ_PAIRED_AGENT={'e' * 64}", (root / ".env").read_text(encoding="utf-8"))
            (root / ".env").write_text(f"KUZZ_OWNER_PUBKEY={NPUB_HEX}\n", encoding="utf-8")
            # Connecting the default profile itself never uses it.
            self.install.pin_default_profile(root, {NPUB_HEX: "a" * 64})
            self.assertNotIn("a" * 64, (root / ".env").read_text(encoding="utf-8"))

    def test_a_busy_gateway_still_installs_and_no_restart_skips_it(self):
        with tempfile.TemporaryDirectory() as tmp, FakeKuzz() as fake,                 mock.patch.object(self.install, "enable_plugin", return_value=True),                 mock.patch.object(self.install, "restart_gateway", return_value=False) as restart:
            home = Path(tmp)
            argv = ["--owner", NPUB, "--hermes-home", str(home), "--cli-path", str(fake.exe)]
            self.assertEqual(self.install.main(argv), 0)
            self.assertEqual(restart.call_count, 1)
            self.assertEqual(self.install.main(argv + ["--no-restart"]), 0)
            self.assertEqual(restart.call_count, 1)

    def test_restart_without_hermes_reports_failure(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(self.install, "find_hermes", return_value=None):
            self.assertFalse(self.install.restart_gateway(Path(tmp)))

    def test_hermes_children_get_no_kuzz_secrets(self):
        with mock.patch.dict(os.environ, {"KUZZ_PRIVATE_KEY": "x", "BUZZ_AUTH_TAG": "y", "PATH_KEEP": "1"}):
            env = self.install.hermes_env(Path("C:/h"))
        self.assertNotIn("KUZZ_PRIVATE_KEY", env)
        self.assertNotIn("BUZZ_AUTH_TAG", env)
        self.assertEqual((env["HERMES_HOME"], env["PATH_KEEP"]), (str(Path("C:/h")), "1"))


if __name__ == "__main__":
    unittest.main()
