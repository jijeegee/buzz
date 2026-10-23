"""Adapter tests against Hermes' real gateway classes and the fake kuzz CLI.

Needs Hermes Agent importable (skipped otherwise), e.g. on Windows:
  set PYTHONPATH=%LOCALAPPDATA%\\hermes\\hermes-agent
  %LOCALAPPDATA%\\hermes\\hermes-agent\\venv\\Scripts\\python -m unittest discover -s plugins/platforms/kuzz/tests
"""

import asyncio
import json
import tempfile
import time
import unittest
from pathlib import Path

from _support import AGENT, CHANNEL, DM, OWNER, STRANGER, FakeKuzz, event_id, watch_line

from kuzz.kuzz_cli import parse_watch_line

try:
    from gateway.config import Platform, PlatformConfig
    from gateway.platform_registry import PlatformEntry, platform_registry

    from kuzz.adapter import BuzzAdapter, register
except ImportError:  # Hermes Agent is not on the path
    BuzzAdapter = None

EYES = "\U0001f440"
NPUB = "npub180cvv07tjdrrgpa0j7j7tmnyl2yr6yr7l8j4s3evf6u64th6gkwsyjh6w6"
NPUB_HEX = "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d"
SECRET = "f" * 64  # a stand-in legacy key; must never appear in calls' args or stdin


class _Ctx:
    """The slice of Hermes' PluginContext.register_platform that the plugin uses."""

    def register_platform(self, name, label, adapter_factory, check_fn, validate_config=None,
                          required_env=None, install_hint="", **kwargs):
        entry = PlatformEntry(
            name=name, label=label, adapter_factory=adapter_factory, check_fn=check_fn,
            validate_config=validate_config, required_env=required_env or [], install_hint=install_hint,
            source="plugin", plugin_name="buzz-platform", **kwargs,
        )
        platform_registry.register(entry, scope=platform_registry.current_scope_key())


@unittest.skipIf(BuzzAdapter is None, "Hermes Agent (gateway package) is not importable")
class AdapterTest(unittest.TestCase):
    def setUp(self):
        self._home = tempfile.TemporaryDirectory()
        self.addCleanup(self._home.cleanup)

    @staticmethod
    def _adapter(**extra):
        # Registered under the test's HERMES_HOME scope; also proves every kwarg is a PlatformEntry field.
        register(_Ctx())
        adapter = BuzzAdapter(PlatformConfig(enabled=True, extra=dict(extra)))
        assert adapter.platform == Platform("buzz")
        return adapter

    def _fake(self, watch, **env):
        values = dict(
            FAKE_KUZZ_SELF=AGENT,
            FAKE_KUZZ_NAME="Hermes",
            FAKE_KUZZ_DMS=[{"dm_id": DM, "participants": [OWNER, AGENT]}],
            FAKE_KUZZ_WATCH=watch,
            KUZZ_ALLOW_ALL_USERS="true",
            BUZZ_ALLOW_ALL_USERS="true",
            HERMES_HOME=self._home.name,
            HERMES_GATEWAY_LOCK_DIR=self._home.name,
        )
        values.update(env)
        fake = FakeKuzz(**values)
        fake._patch.values["KUZZ_CLI_PATH"] = str(fake.exe)
        return fake

    @staticmethod
    def _capture(adapter):
        events = []

        async def capture(event):
            events.append(event)

        adapter.handle_message = capture
        adapter.set_message_handler(lambda event: None)
        return events

    @staticmethod
    async def _until_parked(adapter):
        for _ in range(400):
            if adapter._supervisor and adapter._supervisor.parked:
                return adapter._supervisor.parked
            await asyncio.sleep(0.05)
        raise AssertionError("watch never parked")

    def test_paired_intake_send_and_park(self):
        inbound = [
            watch_line(1, content="@Hermes do X", mentions=[AGENT]),  # mentioned: dispatched
            watch_line(2, content="talking to someone else"),  # no mention in a channel: ignored
            watch_line(3, author=STRANGER, channel=DM, mentions=[AGENT]),  # not allowed, even with allow-all env
            watch_line(4, channel=DM, content="dm without mention"),  # DM: dispatched
            watch_line(5, author=AGENT, mentions=[AGENT]),  # own echo: ignored
            watch_line(1, content="duplicate", mentions=[AGENT]),  # duplicate id: ignored
            watch_line(6, created_at=1_000, mentions=[AGENT]),  # old backlog: ignored
        ]
        watch = [
            {"lines": inbound, "rc": 0},
            {"lines": [], "rc": 3, "stderr": json.dumps({"error": "auth", "message": "signed_out"})},
        ]
        with self._fake(watch, FAKE_KUZZ_OWNER=OWNER) as fake:
            adapter = self._adapter()
            events = self._capture(adapter)
            media = Path(self._home.name) / "chart.png"
            media.write_bytes(b"png")

            async def scenario():
                self.assertTrue(await adapter.connect())
                self.assertEqual(adapter._identity.mode, "paired")
                self.assertEqual((await self._until_parked(adapter))[0], "signed_out")
                self.assertTrue(adapter.has_fatal_error)
                for chat, reply_to in [(CHANNEL, event_id(1)), (DM, event_id(4))]:
                    result = await adapter.send(chat, "answer", reply_to=reply_to)
                    self.assertTrue(result.success, result.error)
                # Owner replies in our thread without a mention: addressed. Replies join the root.
                await adapter._on_message(parse_watch_line(json.dumps(watch_line(7, thread_root=event_id(1)))))
                for kwargs in ({"reply_to": event_id(7)}, {"metadata": {"thread_id": event_id(7)}}):
                    result = await adapter.send(CHANNEL, "again", **kwargs)
                    self.assertTrue(result.success, result.error)
                result = await adapter.send_image_file(CHANNEL, str(media), caption="chart", reply_to=event_id(1))
                self.assertTrue(result.success, result.error)
                missing = await adapter.send_document(CHANNEL, str(media) + ".gone")
                self.assertFalse(missing.success)
                self.assertNotIn(self._home.name, missing.error)
                edited = await adapter.edit_message(CHANNEL, event_id(9), "new text")
                self.assertTrue(edited.success, edited.error)
                self.assertEqual(edited.message_id, event_id(9))
                self.assertTrue(await adapter.delete_message(CHANNEL, event_id(9)))
                self.assertEqual((await adapter.get_chat_info(CHANNEL))["name"], "general")
                await adapter.disconnect()

            asyncio.run(scenario())

            self.assertEqual([e.message_id for e in events], [event_id(1), event_id(4), event_id(7)])
            first, dm, threaded = events
            self.assertEqual(first.text, "do X")
            self.assertEqual(first.source.chat_type, "group")
            self.assertEqual(first.source.user_id, OWNER)
            self.assertIsNone(first.source.thread_id)
            self.assertEqual(dm.source.chat_type, "dm")
            self.assertEqual(threaded.source.thread_id, event_id(1))

            thread = ["--reply-in-thread", event_id(1)]
            self.assertEqual(
                [call["args"] for call in fake.calls("messages", "send")],
                [
                    ["messages", "send", "--channel", CHANNEL, *thread, "--content", "-"],
                    ["messages", "send", "--channel", DM, "--content", "-"],
                    ["messages", "send", "--channel", CHANNEL, *thread, "--content", "-"],
                    ["messages", "send", "--channel", CHANNEL, *thread, "--content", "-"],
                    ["messages", "send", "--channel", CHANNEL, *thread, "--file", str(media), "--content", "-"],
                ],
            )
            seen = [c["args"] for c in fake.calls("reactions", "add")]
            self.assertEqual(seen, [["reactions", "add", "--event", event_id(n), "--emoji", EYES] for n in (1, 4, 7)])
            self.assertEqual(
                fake.calls("messages", "edit")[0]["args"],
                ["messages", "edit", "--event", event_id(9), "--content=new text"],
            )
            self.assertTrue(all(call["env"] == [] for call in fake.calls()), "allow-all or a key reached a kuzz child")
            self.assertEqual(adapter.config.extra["allow_from"], [OWNER])
            self.assertEqual(adapter.config.extra["group_allow_from"], [OWNER])

            cursor = json.loads((Path(self._home.name) / "buzz" / "channel-cursors.json").read_text(encoding="utf-8"))
            self.assertEqual(cursor["identity"], AGENT)
            self.assertEqual(cursor["channels"][DM]["chat_type"], "dm")
            self.assertIn(event_id(1), cursor["channels"][CHANNEL]["seen"])

    def test_saved_cursor_skips_seen_and_older_messages(self):
        last = 2_000_000_000
        cursor_dir = Path(self._home.name) / "buzz"
        cursor_dir.mkdir()
        (cursor_dir / "channel-cursors.json").write_text(
            json.dumps({"identity": AGENT, "relay": "", "channels": {
                CHANNEL: {"chat_type": "group", "last_ts": last, "seen": [event_id(1)]}}}),
            encoding="utf-8",
        )
        inbound = [
            watch_line(1, mentions=[AGENT], created_at=last),  # seen before the restart
            watch_line(2, mentions=[AGENT], created_at=last - 120),  # older than the cursor allows
            watch_line(3, mentions=[AGENT], created_at=last + 1),  # new
            watch_line(4, mentions=[AGENT], channel=DM),  # outside BUZZ_CHANNELS
        ]
        watch = [{"lines": inbound, "rc": 3, "stderr": "revoked"}]
        with self._fake(watch, FAKE_KUZZ_OWNER=OWNER, BUZZ_CHANNELS=CHANNEL) as fake:
            adapter = self._adapter()
            events = self._capture(adapter)

            async def scenario():
                self.assertTrue(await adapter.connect())
                self.assertEqual((await self._until_parked(adapter))[0], "revoked")
                await adapter.disconnect()

            asyncio.run(scenario())
            self.assertEqual([e.message_id for e in events], [event_id(3)])
            # The restart catches up from the cursor (capped at now: these fixtures are in the future).
            args = fake.calls("messages", "watch")[0]["args"]
            self.assertEqual(args[:3], ["messages", "watch", "--since"])
            self.assertLessEqual(int(args[3]), int(time.time()))

    def test_legacy_key_until_paired(self):
        tag = json.dumps(["auth", OWNER, "", "0" * 128])
        inbound = [watch_line(1, author=NPUB_HEX, mentions=[AGENT])]
        watch = [{"lines": inbound, "rc": 3, "stderr": "not_paired"}]
        env = dict(BUZZ_PRIVATE_KEY=SECRET, BUZZ_AUTH_TAG=tag, BUZZ_RELAY_URL="wss://relay.example",
                   BUZZ_ALLOWED_USERS=NPUB, BUZZ_REPLY_IN_THREAD="false")
        with self._fake(watch, **env) as fake:
            adapter = self._adapter()
            events = self._capture(adapter)

            async def scenario():
                self.assertTrue(await adapter.connect())
                self.assertEqual(adapter._identity.mode, "legacy")
                self.assertEqual((await self._until_parked(adapter))[0], "not_paired")
                result = await adapter.send(CHANNEL, "flat", reply_to=event_id(1))
                self.assertTrue(result.success, result.error)
                await adapter.disconnect()

            asyncio.run(scenario())
            self.assertEqual([e.user_id for e in events], [NPUB_HEX])
            self.assertEqual(adapter.config.extra["allow_from"], sorted([OWNER, NPUB_HEX]))
            self.assertEqual(fake.calls("messages", "send")[0]["args"], ["messages", "send", "--channel", CHANNEL, "--content", "-"])
            keyed = ["BUZZ_AUTH_TAG", "BUZZ_PRIVATE_KEY", "KUZZ_AUTH_TAG", "KUZZ_PRIVATE_KEY", "KUZZ_RELAY_URL"]
            self.assertEqual(fake.calls("messages", "watch")[0]["env"], keyed)
            self.assertEqual(fake.calls("auth", "whoami")[0]["env"], [], "the paired probe must run without keys")
            log = (fake.dir / "calls.jsonl").read_text(encoding="utf-8")
            self.assertNotIn(SECRET, log)

    def test_owner_override_and_owner_identity_refused(self):
        for whoami_owner in (OWNER, "null"):  # "null": paired, but the relay has not named the owner yet
            with self._fake([{"rc": 0}], FAKE_KUZZ_OWNER=whoami_owner, KUZZ_OWNER_PUBKEY=STRANGER):
                adapter = self._adapter()

                async def scenario():
                    self.assertTrue(await adapter.connect())
                    self.assertEqual(adapter._identity.mode, "paired")
                    await adapter.disconnect()

                asyncio.run(scenario())
                self.assertEqual(adapter.config.extra["allow_from"], [STRANGER])
        with self._fake([{"rc": 0}], FAKE_KUZZ_OWNER=OWNER, FAKE_KUZZ_SELF=OWNER) as fake:
            adapter = self._adapter()
            self.assertFalse(asyncio.run(adapter.connect()))
            self.assertEqual(adapter.fatal_error_code, "wrong_identity")
            self.assertEqual(fake.calls("messages", "watch"), [])

    def test_not_paired_and_no_key_fails_closed(self):
        with self._fake([{"rc": 0}], BUZZ_RELAY_URL="wss://relay.example") as fake:
            adapter = self._adapter()
            self.assertFalse(asyncio.run(adapter.connect()))
            self.assertEqual(adapter.fatal_error_code, "not_paired")
            self.assertEqual(fake.calls("messages", "watch"), [])


if __name__ == "__main__":
    unittest.main()
