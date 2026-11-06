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
from unittest import mock

from _support import AGENT, CHANNEL, DM, OWNER, STRANGER, FakeKuzz, event_id, watch_line

from kuzz import kuzz_cli
from kuzz.kuzz_cli import parse_watch_line

try:
    from gateway.config import Platform, PlatformConfig
    from gateway.platform_registry import PlatformEntry, platform_registry

    from kuzz import adapter as adapter_module
    from kuzz.adapter import KuzzAdapter, register
except ImportError:  # Hermes Agent is not on the path
    KuzzAdapter = None

EYES = "\U0001f440"
NPUB = "npub180cvv07tjdrrgpa0j7j7tmnyl2yr6yr7l8j4s3evf6u64th6gkwsyjh6w6"
NPUB_HEX = "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d"
SECRET = "f" * 64  # a stand-in legacy key; must never appear in calls' args or stdin
SIBLING = "d" * 64
LISTED = "e" * 64
ANYONE = {"mode": "anyone", "allowlist": []}


class _Ctx:
    """The slice of Hermes' PluginContext.register_platform that the plugin uses."""

    def register_platform(self, name, label, adapter_factory, check_fn, validate_config=None,
                          required_env=None, install_hint="", **kwargs):
        entry = PlatformEntry(
            name=name, label=label, adapter_factory=adapter_factory, check_fn=check_fn,
            validate_config=validate_config, required_env=required_env or [], install_hint=install_hint,
            source="plugin", plugin_name="kuzz-platform", **kwargs,
        )
        platform_registry.register(entry, scope=platform_registry.current_scope_key())


@unittest.skipIf(KuzzAdapter is None, "Hermes Agent (gateway package) is not importable")
class AdapterTest(unittest.TestCase):
    def setUp(self):
        # Hermes may still be writing gateway_state.json right after a disconnect.
        self._home = tempfile.TemporaryDirectory(ignore_cleanup_errors=True)
        self.addCleanup(self._home.cleanup)
        adapter_module._last_park = None
        self.addCleanup(setattr, adapter_module, "_last_park", None)

    @staticmethod
    def _adapter(**extra):
        # Registered under the test's HERMES_HOME scope; also proves every kwarg is a PlatformEntry field.
        register(_Ctx())
        adapter = KuzzAdapter(PlatformConfig(enabled=True, extra=dict(extra)))
        assert adapter.platform == Platform("kuzz")
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
                self.assertEqual((await self._until_parked(adapter))[0], "signed_out")
                self.assertTrue(adapter.has_fatal_error)
                self.assertTrue(adapter.fatal_error_retryable)
                # Right after a park, a reconnect fails at once, so Hermes backs off.
                again = self._adapter()
                self.assertFalse(await again.connect())
                self.assertEqual(again.fatal_error_code, "signed_out")
                self.assertTrue(again.fatal_error_retryable)
                for chat, reply_to in [(CHANNEL, event_id(1)), (DM, event_id(4))]:
                    result = await adapter.send(chat, "answer", reply_to=reply_to)
                    self.assertTrue(result.success, result.error)
                # Outside a DM only a mention counts, even in a thread we answered (as runner agents).
                await adapter._on_message(parse_watch_line(json.dumps(watch_line(7, thread_root=event_id(1)))))
                await adapter._on_message(
                    parse_watch_line(json.dumps(watch_line(8, thread_root=event_id(1), mentions=[AGENT]))))
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

            self.assertEqual([e.message_id for e in events], [event_id(1), event_id(4), event_id(8)])
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
                    ["messages", "send", "--channel", CHANNEL, "--content", "-"],  # asked on the timeline: flat
                    ["messages", "send", "--channel", DM, "--content", "-"],
                    ["messages", "send", "--channel", CHANNEL, *thread, "--content", "-"],
                    ["messages", "send", "--channel", CHANNEL, *thread, "--content", "-"],
                    ["messages", "send", "--channel", CHANNEL, "--file", str(media), "--content", "-"],
                ],
            )
            seen = [c["args"] for c in fake.calls("reactions", "add")]
            self.assertEqual(seen, [["reactions", "add", "--event", event_id(n), "--emoji", EYES] for n in (1, 4, 8)])
            self.assertEqual(
                fake.calls("messages", "edit")[0]["args"],
                ["messages", "edit", "--event", event_id(9), "--content=new text"],
            )
            # Past the first whoami each call names the agent; nothing else reaches a kuzz child.
            self.assertTrue(all(call["env"] in ([], ["KUZZ_PAIRED_AGENT"]) for call in fake.calls()),
                            "allow-all or a key reached a kuzz child")
            self.assertEqual(adapter.config.extra["allow_from"], [OWNER])
            self.assertEqual(adapter.config.extra["group_allow_from"], [OWNER])

            cursor = json.loads((Path(self._home.name) / "kuzz" / "channel-cursors.json").read_text(encoding="utf-8"))
            self.assertEqual(cursor["identity"], AGENT)
            self.assertEqual(cursor["channels"][DM]["chat_type"], "dm")
            self.assertIn(event_id(1), cursor["channels"][CHANNEL]["seen"])

    def test_saved_cursor_skips_seen_and_older_messages(self):
        last = 2_000_000_000
        cursor_dir = Path(self._home.name) / "kuzz"
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
            watch_line(4, mentions=[AGENT], channel=DM),  # outside KUZZ_CHANNELS
        ]
        watch = [{"lines": inbound, "rc": 3, "stderr": "revoked"}]
        with self._fake(watch, FAKE_KUZZ_OWNER=OWNER, KUZZ_CHANNELS=CHANNEL) as fake:
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
            self.assertEqual(args[:3] + args[4:5], ["messages", "watch", "--access-owner", "--since"])
            self.assertLessEqual(int(args[5]), int(time.time()))

    def test_buzz_settings_belong_to_buzz(self):
        env = dict(BUZZ_PRIVATE_KEY=SECRET, BUZZ_AUTH_TAG="tag", BUZZ_RELAY_URL="wss://buzz.example",
                   BUZZ_CHANNELS="other", BUZZ_HOME_CHANNEL="buzz-home", BUZZ_ALLOWED_USERS="f" * 64,
                   BUZZ_REACTION_ONLY_USERS="f" * 64, BUZZ_REQUIRE_MENTION="false", BUZZ_OWNER_PUBKEY="f" * 64)
        with self._fake([{"rc": 0}], FAKE_KUZZ_OWNER=OWNER, **env) as fake:
            adapter = self._adapter()
            self.assertEqual(adapter.channels, set())
            self.assertEqual(adapter.home_channel, "")
            self.assertTrue(adapter.require_mention)
            self.assertEqual(adapter._reaction_only, set())

            async def scenario():
                self.assertTrue(await adapter.connect())
                await adapter.disconnect()

            asyncio.run(scenario())
            self.assertEqual(adapter.config.extra["allow_from"], [OWNER])
            self.assertNotIn(SECRET, (fake.dir / "calls.jsonl").read_text(encoding="utf-8"))

    def _sender_run(self, fake, adapter, lines, *, settle=None):
        """Feed ``lines`` to a connected adapter; ``settle`` runs before the lookups finish."""

        async def scenario():
            self.assertTrue(await adapter.connect())
            for line in lines:
                await adapter._on_message(parse_watch_line(json.dumps(line)))
            if settle:
                await settle()
            while adapter._admissions:
                await asyncio.sleep(0.01)
            await adapter.disconnect()

        asyncio.run(scenario())

    def test_owners_other_agents_are_heard_like_the_owner(self):
        sibling, other = "d" * 64, "e" * 64
        lines = [
            watch_line(1, author=sibling, content="@Hermes first", mentions=[AGENT]),
            watch_line(2, author=sibling, content="@Hermes second", mentions=[AGENT]),
            watch_line(3, author=sibling, channel=DM, content="dm"),
            watch_line(4, author=sibling, content="not for us"),
            watch_line(5, author=OWNER, content="@Hermes hi", mentions=[AGENT]),
            watch_line(6, author=other, content="@Hermes hello", mentions=[AGENT]),
            watch_line(7, author=other, content="@Hermes again", mentions=[AGENT]),
        ]
        with self._fake([{"rc": 0}], FAKE_KUZZ_OWNER=OWNER, FAKE_KUZZ_SAME_OWNER=[sibling]) as fake:
            adapter = self._adapter()
            events = self._capture(adapter)
            self._sender_run(fake, adapter, lines)
            self.assertTrue(adapter.authorization_is_upstream)
            self.assertEqual({e.text for e in events}, {"first", "second", "dm", "hi"})
            self.assertFalse(any(e.source.is_bot for e in events))
            # A lookup per sender at a time, none for a message that is not for us or from the owner.
            lookups = [call["args"][3] for call in fake.calls("agents", "same-owner")]
            self.assertEqual(lookups.count(sibling), 1)
            self.assertEqual(lookups.count(other), 1)
            self.assertNotIn(OWNER, lookups)

    def test_answers_are_remembered_for_a_while(self):
        sibling, other = "d" * 64, "e" * 64
        with self._fake([{"rc": 0}], FAKE_KUZZ_OWNER=OWNER, FAKE_KUZZ_SAME_OWNER=[sibling]) as fake:
            adapter = self._adapter()
            events = self._capture(adapter)

            async def say(n, author):
                await adapter._on_message(parse_watch_line(json.dumps(
                    watch_line(n, author=author, content=f"m{n}", mentions=[AGENT]))))
                while adapter._admissions:
                    await asyncio.sleep(0.01)

            def expire(author, seconds):
                same, until = adapter._same_owner_answers[author]
                adapter._same_owner_answers[author] = (same, until - seconds)

            async def scenario():
                self.assertTrue(await adapter.connect())
                await say(1, sibling)
                await say(2, sibling)
                await say(3, other)
                await say(4, other)
                self.assertEqual(len(fake.calls("agents", "same-owner")), 2)
                expire(other, 50)
                await say(5, other)
                self.assertEqual(len(fake.calls("agents", "same-owner")), 2)
                expire(other, 20)
                expire(sibling, 290)
                await say(6, other)
                await say(7, sibling)
                self.assertEqual(len(fake.calls("agents", "same-owner")), 3)
                expire(sibling, 20)
                await say(8, sibling)
                self.assertEqual(len(fake.calls("agents", "same-owner")), 4)
                await adapter.disconnect()

            asyncio.run(scenario())
            self.assertEqual([e.text for e in events], ["m1", "m2", "m7", "m8"])

    def test_a_failed_lookup_denies_and_is_not_remembered(self):
        sibling = "d" * 64
        lines = [watch_line(1, author=sibling, content="@Hermes hi", mentions=[AGENT])]
        with self._fake([{"rc": 0}], FAKE_KUZZ_OWNER=OWNER) as fake:  # the call fails
            adapter = self._adapter()
            events = self._capture(adapter)
            self._sender_run(fake, adapter, lines)
            self.assertEqual(events, [])
            self.assertEqual(len(adapter._same_owner_answers), 0)
        with self._fake([{"rc": 0}], FAKE_KUZZ_OWNER=OWNER, FAKE_KUZZ_SAME_OWNER=[sibling],
                        FAKE_KUZZ_SAME_OWNER_DELAY="5") as fake:  # the call is too slow
            adapter = self._adapter()
            events = self._capture(adapter)
            with mock.patch.object(adapter_module, "_SAME_OWNER_TIMEOUT_SECS", 0.3):
                self._sender_run(fake, adapter, lines)
            self.assertEqual(events, [])
            self.assertEqual(len(adapter._same_owner_answers), 0)

    def test_a_slow_lookup_does_not_hold_back_the_owner(self):
        sibling = "d" * 64
        lines = [
            watch_line(1, author=sibling, content="@Hermes slow", mentions=[AGENT]),
            watch_line(2, author=OWNER, content="@Hermes fast", mentions=[AGENT]),
        ]
        with self._fake([{"rc": 0}], FAKE_KUZZ_OWNER=OWNER, FAKE_KUZZ_SAME_OWNER=[sibling],
                        FAKE_KUZZ_SAME_OWNER_DELAY="0.5") as fake:
            adapter = self._adapter()
            events = self._capture(adapter)

            async def settle():
                self.assertEqual([e.text for e in events], ["fast"])

            self._sender_run(fake, adapter, lines, settle=settle)
            self.assertEqual([e.text for e in events], ["fast", "slow"])

    def test_disconnect_leaves_an_unanswered_message_for_the_replay(self):
        sibling = "d" * 64
        line = watch_line(1, author=sibling, content="@Hermes hi", mentions=[AGENT])
        with self._fake([{"rc": 0}], FAKE_KUZZ_OWNER=OWNER, FAKE_KUZZ_SAME_OWNER=[sibling],
                        FAKE_KUZZ_SAME_OWNER_DELAY="5"):
            adapter = self._adapter()
            events = self._capture(adapter)

            async def scenario():
                self.assertTrue(await adapter.connect())
                await adapter._on_message(parse_watch_line(json.dumps(line)))
                await adapter.disconnect()

            asyncio.run(scenario())
            self.assertEqual(events, [])
            self.assertFalse(adapter._admissions)
            self.assertNotIn(event_id(1), adapter._channel(CHANNEL)["seen"])

    def test_disconnect_after_delivery_keeps_the_message_seen(self):
        sibling = "d" * 64
        line = watch_line(1, author=sibling, content="@Hermes hi", mentions=[AGENT])
        with self._fake([{"rc": 0}], FAKE_KUZZ_OWNER=OWNER, FAKE_KUZZ_SAME_OWNER=[sibling]):
            adapter = self._adapter()
            events = self._capture(adapter)

            async def slow_reaction(*_):
                await asyncio.sleep(5)

            adapter.send_reaction = slow_reaction

            async def scenario():
                self.assertTrue(await adapter.connect())
                await adapter._on_message(parse_watch_line(json.dumps(line)))
                while not events:
                    await asyncio.sleep(0.01)
                await asyncio.sleep(0)
                await adapter.disconnect()

            asyncio.run(scenario())
            self.assertEqual(len(events), 1)
            self.assertIn(event_id(1), adapter._channel(CHANNEL)["seen"])

    def _heard(self, access, lines, *, prepare=None, **env):
        """The events delivered from ``lines`` under ``access``. ``prepare`` gets the connected adapter."""
        now = int(time.time())
        # Each run starts clean, or the cursor of the one before would drop these same ids.
        Path(self._home.name, "kuzz", "channel-cursors.json").unlink(missing_ok=True)
        with self._fake([{"rc": 0}], FAKE_KUZZ_OWNER=OWNER, FAKE_KUZZ_SAME_OWNER=[SIBLING], **env) as fake:
            adapter = self._adapter()
            events = self._capture(adapter)
            self._keep_access(adapter)

            async def scenario():
                self.assertTrue(await adapter.connect())
                adapter._on_access(kuzz_cli.parse_access_line(json.dumps({"type": "access", **access})))
                if prepare:
                    prepare(adapter)
                for line in lines:
                    while adapter._admissions:
                        await asyncio.sleep(0.01)
                    await adapter._on_message(parse_watch_line(json.dumps({"created_at": now, **line})))
                while adapter._admissions:
                    await asyncio.sleep(0.01)
                await adapter.disconnect()

            asyncio.run(scenario())
            self.calls = fake.calls()
        return events

    @staticmethod
    def _keep_access(adapter):
        """The fake watch ends and restarts: without this each start resets the setting. -> The real hook."""
        reset = adapter._on_watch_start

        async def keep():
            pass

        adapter._on_watch_start = keep
        return reset

    def _said_now(self, n):
        return {**self._said(n), "created_at": int(time.time())}

    @staticmethod
    def _said(n, author=STRANGER, **overrides):
        line = watch_line(n, author=author, content=f"m{n}", mentions=[AGENT], **overrides)
        del line["created_at"]
        return line

    def test_who_is_heard_in_each_mode(self):
        modes = {
            "owner-only": ({"mode": "owner-only", "allowlist": []}, {"owner", "sibling"}),
            "allowlist": ({"mode": "allowlist", "allowlist": [LISTED]}, {"owner", "sibling", "listed"}),
            "anyone": ({"mode": "anyone", "allowlist": []}, {"owner", "sibling", "listed", "stranger"}),
        }
        senders = {"owner": OWNER, "sibling": SIBLING, "listed": LISTED, "stranger": STRANGER}
        for mode, (access, heard) in modes.items():
            with self.subTest(mode):
                lines = [{**self._said(n, author), "content": f"@Hermes {who}"} for n, (who, author) in enumerate(senders.items(), 1)]
                events = self._heard(access, lines)
                self.assertEqual({e.text for e in events}, heard)
                control = {e.text: e.allow_gateway_control for e in events}
                self.assertTrue(control.pop("owner") and control.pop("sibling"))
                self.assertFalse(any(control.values()), "other people never get gateway control")

    def test_other_people_must_mention_even_without_require_mention(self):
        line = self._said(1)
        line["mentions"] = []
        self.assertEqual(self._heard(ANYONE, [line], KUZZ_REQUIRE_MENTION="false"), [])

    def test_a_dm_stays_closed_to_other_people(self):
        for name, env in (("dm", {"FAKE_KUZZ_CHANNEL_TYPE": "dm"}), ("no type", {"FAKE_KUZZ_CHANNEL_TYPE": ""}),
                          ("failed", {"FAKE_KUZZ_CHANNEL_TYPE": "fail"}), ("workflow", {"FAKE_KUZZ_CHANNEL_TYPE": "workflow"})):
            with self.subTest(name):
                # The DM list doesn't know this channel: a hidden DM, or past its limit.
                self.assertEqual(self._heard(ANYONE, [self._said(1)], **env), [])
        self.assertEqual(self._heard(ANYONE, [self._said(1, channel=DM)], FAKE_KUZZ_CHANNEL_TYPE="dm"), [])

    def test_a_channel_type_is_looked_up_once_and_a_failure_again(self):
        self.assertEqual(len(self._heard(ANYONE, [self._said(1), self._said(2)])), 2)
        self.assertEqual(sum(call["args"][:2] == ["channels", "get"] for call in self.calls), 1)
        self._heard(ANYONE, [self._said(1), self._said(2)], FAKE_KUZZ_CHANNEL_TYPE="fail")
        self.assertEqual(sum(call["args"][:2] == ["channels", "get"] for call in self.calls), 2)

    def test_allowed_users_only_narrows(self):
        self.assertEqual({e.text for e in self._heard(ANYONE, [self._said(1, LISTED), self._said(2)], KUZZ_ALLOWED_USERS=LISTED)},
                         {"m1"})
        owner_only = {"mode": "owner-only", "allowlist": []}
        self.assertEqual(self._heard(owner_only, [self._said(1, LISTED)], KUZZ_ALLOWED_USERS=LISTED), [])
        self.assertEqual(self._heard({"mode": "allowlist", "allowlist": []}, [self._said(1, LISTED)],
                                     KUZZ_ALLOWED_USERS=LISTED), [])

    def test_allowed_users_with_only_typos_lets_nobody_in(self):
        self.assertEqual(self._heard(ANYONE, [self._said(1)], KUZZ_ALLOWED_USERS="npub1typo"), [])

    def test_a_burst_shares_one_channel_type_lookup(self):
        def burst(adapter):
            first = adapter._on_message

            async def together(msg):
                await first(msg)
                await first(parse_watch_line(json.dumps(self._said_now(2))))

            adapter._on_message = together

        self.assertEqual(len(self._heard(ANYONE, [self._said(1)], prepare=burst)), 2)
        self.assertEqual(sum(call["args"][:2] == ["channels", "get"] for call in self.calls), 1)

    def test_a_late_message_from_other_people_is_dropped(self):
        late = int(time.time()) - adapter_module._OTHERS_MAX_AGE_SECS - 30
        lines = [{**self._said(1), "created_at": late}, {**self._said(2, OWNER), "created_at": late},
                 {**self._said(3, SIBLING), "created_at": late}]
        self.assertEqual({e.text for e in self._heard(ANYONE, lines)}, {"m2", "m3"})

    def test_narrowing_during_a_lookup_wins(self):
        def narrow_in_the_lookup(adapter):
            async def lookup(_msg):
                adapter._on_access(kuzz_cli.WatchAccess())
                return True

            adapter._in_open_channel = lookup

        self.assertEqual(self._heard(ANYONE, [self._said(1)], prepare=narrow_in_the_lookup), [])

    def test_a_setting_applies_live_and_a_new_watch_starts_owner_only(self):
        async def scenario(adapter, events, watch_start):
            self.assertTrue(await adapter.connect())
            adapter._on_access(kuzz_cli.WatchAccess("anyone"))
            await watch_start()
            self.assertEqual(adapter._access, kuzz_cli.WatchAccess())
            await adapter._on_message(parse_watch_line(json.dumps({**self._said(1), "created_at": int(time.time())})))
            while adapter._admissions:
                await asyncio.sleep(0.01)
            self.assertEqual(events, [])
            adapter._on_access(kuzz_cli.WatchAccess("anyone"))
            await adapter._on_message(parse_watch_line(json.dumps({**self._said(2), "created_at": int(time.time())})))
            while adapter._admissions:
                await asyncio.sleep(0.01)
            self.assertEqual([e.text for e in events], ["m2"])
            await adapter.disconnect()

        with self._fake([{"rc": 0}], FAKE_KUZZ_OWNER=OWNER, FAKE_KUZZ_SAME_OWNER=[]):
            adapter = self._adapter()
            asyncio.run(scenario(adapter, self._capture(adapter), self._keep_access(adapter)))

    def test_older_hermes_keeps_other_people_out(self):
        with mock.patch.object(adapter_module, "_CAN_LIMIT_CONTROL", False):
            self.assertEqual({e.text for e in self._heard(ANYONE, [self._said(1), self._said(2, OWNER)])}, {"m2"})

    def test_a_sender_in_the_future_leaves_the_cursor_at_now(self):
        ahead = int(time.time()) + 900
        before = int(time.time())
        with self._fake([{"rc": 0}], FAKE_KUZZ_OWNER=OWNER, FAKE_KUZZ_SAME_OWNER=[]):
            adapter = self._adapter()
            self._capture(adapter)

            async def scenario():
                self.assertTrue(await adapter.connect())
                await adapter._on_message(parse_watch_line(json.dumps({**self._said(1), "created_at": ahead})))
                while adapter._admissions:
                    await asyncio.sleep(0.01)
                await adapter.disconnect()

            asyncio.run(scenario())
            self.assertLess(adapter._channel(CHANNEL)["last_ts"], before + 60)

    def test_reaction_only_sender_still_gets_the_seen_reaction(self):
        lines = [watch_line(1, author=STRANGER, content="@Hermes hi", mentions=[AGENT])]
        with self._fake([{"rc": 0}], FAKE_KUZZ_OWNER=OWNER, FAKE_KUZZ_SAME_OWNER=[],
                        KUZZ_REACTION_ONLY_USERS=STRANGER) as fake:
            adapter = self._adapter()
            events = self._capture(adapter)
            self._sender_run(fake, adapter, lines)
            self.assertEqual(events, [])
            self.assertTrue(any("react" in " ".join(call["args"]) for call in fake.calls()))

    def test_owner_override_and_owner_identity_refused(self):
        for whoami_owner in (OWNER, "null"):  # "null": paired, but the relay has not named the owner yet
            with self._fake([{"rc": 0}], FAKE_KUZZ_OWNER=whoami_owner, KUZZ_OWNER_PUBKEY=STRANGER):
                adapter = self._adapter()

                async def scenario():
                    self.assertTrue(await adapter.connect())
                    await adapter.disconnect()

                asyncio.run(scenario())
                self.assertEqual(adapter.config.extra["allow_from"], [STRANGER])
        with self._fake([{"rc": 0}], FAKE_KUZZ_OWNER=OWNER, FAKE_KUZZ_SELF=OWNER) as fake:
            adapter = self._adapter()
            self.assertFalse(asyncio.run(adapter.connect()))
            self.assertEqual(adapter.fatal_error_code, "wrong_identity")
            self.assertEqual(fake.calls("messages", "watch"), [])

    def test_connect_during_an_update_fails_retryably(self):
        with mock.patch.dict("os.environ", {"LOCALAPPDATA": self._home.name}):
            signal = Path(self._home.name) / "kuzz" / "update-pause"
            signal.parent.mkdir()
            signal.touch()
            with FakeKuzz(FAKE_KUZZ_SELF=AGENT, FAKE_KUZZ_OWNER=OWNER) as fake:
                with mock.patch.object(adapter_module, "CONNECT_UPDATE_HOLD", 0.0):
                    adapter = self._adapter()
                    self.assertFalse(asyncio.run(adapter.connect()))
                self.assertEqual(fake.calls(), [])
            self.assertTrue(adapter.fatal_error_retryable)

    def test_a_refused_relay_fails_connect_before_the_watch(self):
        with self._fake([{"rc": 0}], FAKE_KUZZ_OWNER=OWNER, FAKE_KUZZ_DMS_REFUSED="1") as fake:
            adapter = self._adapter()
            self.assertFalse(asyncio.run(adapter.connect()))
            self.assertEqual(adapter.fatal_error_code, "relay_refused")
            self.assertIn("restricted: device", adapter.fatal_error_message)
            # Retryable, so Hermes backs off; the watch never starts.
            self.assertTrue(adapter.fatal_error_retryable)
            self.assertEqual(fake.calls("messages", "watch"), [])

    def test_not_paired_fails_closed_even_with_a_buzz_key(self):
        with self._fake([{"rc": 0}], BUZZ_RELAY_URL="wss://relay.example", BUZZ_PRIVATE_KEY=SECRET) as fake:
            adapter = self._adapter()
            self.assertFalse(asyncio.run(adapter.connect()))
            self.assertEqual(adapter.fatal_error_code, "not_paired")
            # Retryable, or Hermes stops a gateway whose other platforms are still connecting.
            self.assertTrue(adapter.fatal_error_retryable)
            self.assertEqual(fake.calls("messages", "watch"), [])


@unittest.skipIf(KuzzAdapter is None, "Hermes Agent (gateway package) is not importable")
class RegistrationTest(unittest.TestCase):
    def _entry(self):
        captured = {}

        class Ctx:
            def register_platform(self, **kwargs):
                captured.update(kwargs)

        register(Ctx())
        return captured

    def test_it_is_its_own_platform(self):
        entry = self._entry()
        self.assertEqual((entry["name"], entry["label"]), ("kuzz", "Kuzz"))
        self.assertEqual(entry["cron_deliver_env_var"], "KUZZ_HOME_CHANNEL")
        self.assertNotIn("allowed_users_env", entry)  # would deny the owner where core checks the list

    def test_a_newer_hermes_gets_media_and_channel_targets(self):
        entry = self._entry()
        self.assertTrue(entry["standalone_send"].media)
        self.assertFalse(entry["standalone_send"].all_sends)
        channel = "01234567-89ab-cdef-0123-456789abcdef"
        self.assertEqual(entry["target_ref_syntax_fn"](channel)[0], channel)

    def test_an_older_hermes_loads_without_them(self):
        with mock.patch.object(adapter_module, "fields", return_value=[]):
            entry = self._entry()
        self.assertNotIn("standalone_send", entry)
        self.assertNotIn("target_ref_syntax_fn", entry)

    def test_a_cli_alone_is_not_a_connection(self):
        with FakeKuzz() as fake, mock.patch.dict("os.environ", {"KUZZ_CLI_PATH": str(fake.exe)}):
            with mock.patch.dict("os.environ"):
                import os

                os.environ.pop("KUZZ_OWNER_PUBKEY", None)
                self.assertFalse(adapter_module.validate_config(PlatformConfig(enabled=True)))
                self.assertIsNone(adapter_module._env_enablement())
            with mock.patch.dict("os.environ", {"KUZZ_OWNER_PUBKEY": OWNER}):
                self.assertTrue(adapter_module.validate_config(PlatformConfig(enabled=True)))
                self.assertEqual(adapter_module._env_enablement()["owner"], OWNER)


@unittest.skipIf(KuzzAdapter is None, "Hermes Agent (gateway package) is not importable")
class SecondaryProfileTest(unittest.TestCase):
    """Inside a multiplexed secondary profile: its own .env counts, the process env never does."""

    home = Path("hermes/profiles/work")

    def scoped(self, secrets):
        self.env = mock.patch.dict("os.environ", {
            "KUZZ_PAIRED_AGENT": "d" * 64, "KUZZ_OWNER_PUBKEY": "e" * 64, "KUZZ_HOME_CHANNEL": "default-home",
        })
        patches = [self.env, mock.patch.object(adapter_module, "_profile_scoped", return_value=True),
                   mock.patch.object(adapter_module, "_get_secret", side_effect=lambda name, default=None: secrets.get(name, default)),
                   mock.patch.object(adapter_module, "check_requirements", return_value=True),
                   mock.patch.object(adapter_module, "_hermes_home", side_effect=lambda: self.home)]
        for patcher in patches:
            patcher.start()
            self.addCleanup(patcher.stop)

    def identity(self):
        calls = []

        async def whoami(cli, args, env):
            calls.append(env)
            return {"mode": "paired", "pubkey": env.get("KUZZ_PAIRED_AGENT") or "c" * 64, "owner": OWNER}

        with mock.patch.object(adapter_module, "_cli_json", whoami):
            return asyncio.run(adapter_module.resolve_identity(["kuzz"], {})), calls

    def test_it_acts_as_its_pinned_agent(self):
        pin = "a" * 64
        self.scoped({"KUZZ_PAIRED_AGENT": pin, "KUZZ_OWNER_PUBKEY": OWNER})
        identity, [env] = self.identity()
        self.assertEqual(env["KUZZ_PAIRED_AGENT"], pin)
        self.assertEqual((identity.self_pubkey, identity.owner), (pin, OWNER))
        self.assertEqual(identity.env_add, {"KUZZ_PAIRED_AGENT": pin})

    def test_without_a_pin_it_is_not_paired(self):
        self.scoped({"KUZZ_OWNER_PUBKEY": OWNER})
        identity, calls = self.identity()
        self.assertIsNone(identity)
        self.assertEqual(calls, [])

    def test_the_default_profile_in_its_own_scope_keeps_the_newest_pairing(self):
        self.home = Path("hermes")
        self.scoped({"KUZZ_OWNER_PUBKEY": OWNER})
        identity, [env] = self.identity()
        # Named from then on, so a later pairing can't take over.
        self.assertEqual(identity.env_add, {"KUZZ_PAIRED_AGENT": identity.self_pubkey})
        self.assertTrue(identity.self_pubkey)

    def test_it_is_enabled_from_its_own_env_only(self):
        self.scoped({"KUZZ_OWNER_PUBKEY": OWNER, "KUZZ_HOME_CHANNEL": "own-home"})
        self.assertEqual(adapter_module._env_enablement(),
                         {"owner": OWNER, "home_channel": {"chat_id": "own-home", "name": "own-home"}})
        self.assertEqual(adapter_module._setting(("KUZZ_HOME_CHANNEL",), {}, "home_channel"), "own-home")

    def test_a_profile_never_connected_is_not_enabled(self):
        self.scoped({})
        self.assertIsNone(adapter_module._env_enablement())
        self.assertEqual(adapter_module._setting(("KUZZ_HOME_CHANNEL",), {"home_channel": "x"}, "home_channel"), "x")


if __name__ == "__main__":
    unittest.main()
