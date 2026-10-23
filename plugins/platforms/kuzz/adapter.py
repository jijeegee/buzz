"""Buzz platform adapter for Hermes Agent, rebuilt on the Kuzz CLI (device-bound keys).

A drop-in replacement for Hermes' bundled ``buzz`` platform: the same platform
name, the same ``BUZZ_*`` env vars and ``gateway.platforms.buzz.extra`` keys,
the same cursor file (``$HERMES_HOME/buzz/channel-cursors.json``) and the same
``buzz`` binary lookup, falling back to ``kuzz``. Its manifest name equals the
bundled plugin's (``buzz-platform``), so once installed in
``$HERMES_HOME/plugins/`` Hermes loads this copy instead of the bundled one.

Transport: a supervised ``kuzz messages watch`` (every channel and DM, new ones
included) replaces the WebSocket/poll loop. Replies, edits, deletes, reactions
and files go through ``kuzz messages send|edit|delete`` and ``kuzz reactions add``.

Identity:
- paired: ``kuzz auth whoami`` reports an ``owner``. Every child runs without
  keys and the CLI signs with the identity bound to this computer.
- legacy: not paired, but BUZZ_PRIVATE_KEY (or BUZZ_CREDENTIALS_FILE) is set.
  The key reaches the CLI through the child env as before. Kuzz refuses this
  once device keys are enforced, so a warning asks to run ``kuzz pair``.
- neither: the adapter does not start.

Access: the owner (``KUZZ_OWNER_PUBKEY``, else the paired owner, else the
NIP-OA auth tag's owner) is always allowed; BUZZ_ALLOWED_USERS adds people.
There is no allow-all: BUZZ_ALLOW_ALL_USERS / KUZZ_ALLOW_ALL_USERS are ignored
and removed from every child's environment. Keys are never logged.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import time
from collections import OrderedDict
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Sequence, Set, Tuple

from gateway.config import Platform
from gateway.platforms.base import BasePlatformAdapter, SendResult
from gateway.platforms.event import MessageEvent, MessageType

from . import kuzz_cli
from .kuzz_cli import WatchMessage, WatchSupervisor, normalize_pubkey

try:
    from gateway.platforms._shared import profile_scoped as _profile_scoped
except Exception:  # older Hermes

    def _profile_scoped() -> bool:
        return False


try:
    from agent.secret_scope import UnscopedSecretError as _UnscopedSecretError
    from agent.secret_scope import get_secret as _get_secret
except Exception:  # older Hermes
    _get_secret, _UnscopedSecretError = None, Exception


def _secret(name: str) -> str:
    """Profile-aware secret read, as the bundled plugin does: a secondary profile's own .env, else the env."""
    value = None
    if _get_secret is not None:
        try:
            value = _get_secret(name, None)
        except _UnscopedSecretError:
            value = None
    if value is None and not _profile_scoped():
        value = os.getenv(name)
    return str(value or "").strip()


logger = logging.getLogger(__name__)

PLATFORM_NAME = "buzz"
_SEEN_CAP = 500  # per channel, as in the bundled plugin's cursor file
_ID_CAP = 2000
# A channel without a saved cursor ignores messages this much older than the connect time.
_BACKLOG_GRACE_SECS = 600
# A channel with a saved cursor ignores messages this much older than its last one.
_REPLAY_SLACK_SECS = 60
# Longest catch-up after a restart (`kuzz messages watch --since`).
_MAX_CATCH_UP_SECS = 86400
_DM_REFRESH_SECS = 60.0
_CURSOR_SAVE_SECS = 5.0
# `kuzz messages edit` takes the text as an argument (no stdin); stay under the Windows limit.
_EDIT_ARG_LIMIT = 24000
_SEEN_REACTION = "\U0001f440"
_CURSOR_SUBDIR, _CURSOR_FILENAME = "buzz", "channel-cursors.json"
_KEY_FIELDS = ("nsec", "private_key_hex", "private_key")
_MISSING = object()


# ── Settings ────────────────────────────────────────────────────────────────


def _setting(names: Sequence[str], extra: dict, key: str, default: Any = None) -> Any:
    """Env (first non-empty of ``names``) overrides ``extra[key]``. Under a secondary
    profile the process env belongs to the default profile, so only ``extra`` counts."""
    if not _profile_scoped():
        for name in names:
            value = os.getenv(name)
            if value is not None and value.strip():
                return value.strip()
    return extra.get(key, default)


def _csv(raw: Any) -> List[str]:
    if isinstance(raw, (list, tuple, set)):
        return [str(item).strip() for item in raw if str(item).strip()]
    return [part.strip() for part in str(raw or "").split(",") if part.strip()]


def _pubkeys(raw: Iterable[str]) -> Set[str]:
    return {pk for pk in (normalize_pubkey(ref) for ref in raw) if pk}


def _flag(raw: Any, default: bool) -> bool:
    if raw is None or raw == "":
        return default
    return str(raw).strip().lower() not in ("false", "0", "no", "off")


def _reply_to_mode(config, extra: dict) -> str:
    """Same rule as the bundled plugin: "first"/"all" thread, "off" posts flat."""
    mode = str(os.getenv("BUZZ_REPLY_TO_MODE") or getattr(config, "reply_to_mode", "first") or "first").strip().lower()
    rit = os.getenv("BUZZ_REPLY_IN_THREAD")
    if rit is None:
        rit = extra.get("reply_in_thread")
    return "off" if rit is not None and not _flag(rit, True) else mode


def _relay_url(extra: dict) -> str:
    return str(_setting(("BUZZ_RELAY_URL",), extra, "relay_url", "") or "").strip()


def _cli_path(extra: dict) -> str:
    """KUZZ_CLI_PATH, then BUZZ_CLI_PATH / cli_path, then buzz, ~/bin/buzz, kuzz."""
    return kuzz_cli.resolve_cli_path(
        os.getenv("KUZZ_CLI_PATH", ""), str(_setting(("BUZZ_CLI_PATH",), extra, "cli_path", "") or "")
    )


def _legacy_credentials(extra: dict) -> Tuple[str, str]:
    """(raw key, auth tag) from BUZZ_PRIVATE_KEY / BUZZ_AUTH_TAG or the credentials file. Never logged."""
    key = _secret("BUZZ_PRIVATE_KEY")
    tag = _secret("BUZZ_AUTH_TAG")
    path = _secret("BUZZ_CREDENTIALS_FILE") or str(extra.get("credentials_file", "") or "").strip()
    if not key and path:
        try:
            data = json.loads(Path(path).expanduser().read_text(encoding="utf-8"))
        except (OSError, ValueError):
            data = None
        if isinstance(data, dict):
            key = next((data[f].strip() for f in _KEY_FIELDS if isinstance(data.get(f), str) and data[f].strip()), "")
            raw_tag = data.get("auth_tag")
            if not tag and raw_tag:
                tag = raw_tag if isinstance(raw_tag, str) else json.dumps(raw_tag, separators=(",", ":"))
    return key, tag


def _auth_tag_owner(tag: str) -> Optional[str]:
    """Owner pubkey of a NIP-OA ``["auth", <owner>, <conditions>, <sig>]`` tag."""
    try:
        data = json.loads(tag)
    except ValueError:
        return None
    return normalize_pubkey(data[1]) if isinstance(data, list) and len(data) > 1 else None


@dataclass(frozen=True)
class Identity:
    mode: str  # "paired" | "legacy"
    env_add: Dict[str, str] = field(default_factory=dict)
    self_pubkey: str = ""
    owner: Optional[str] = None


async def _cli_json(cli: Sequence[str], args: List[str], env: Dict[str, str]) -> Any:
    try:
        rc, out, err = await kuzz_cli.run_cli(cli, args, env=env)
    except OSError:
        return None
    if rc != 0:
        logger.debug("Buzz: `%s` failed: %s", " ".join(args[:2]), kuzz_cli.cli_error_message(err, rc))
        return None
    try:
        return json.loads(out)
    except ValueError:
        return None


async def resolve_identity(cli: Sequence[str], extra: dict) -> Optional[Identity]:
    """Paired identity if the CLI reports an owner, else legacy raw-key mode if configured, else None."""
    configured_owner = normalize_pubkey(_setting(("KUZZ_OWNER_PUBKEY",), extra, "owner"))
    me = await _cli_json(cli, ["auth", "whoami"], kuzz_cli.child_env())
    # A paired CLI answers {"mode": "paired", "pubkey", "owner", ...}; owner may be null until the relay says.
    if isinstance(me, dict) and me.get("mode") == "paired" and normalize_pubkey(me.get("pubkey")):
        return Identity(
            "paired", {}, normalize_pubkey(me.get("pubkey")) or "", configured_owner or normalize_pubkey(me.get("owner"))
        )
    key, tag = _legacy_credentials(extra)
    if not key:
        return None
    add = {"KUZZ_PRIVATE_KEY": key, "BUZZ_PRIVATE_KEY": key}
    if tag:
        add.update(KUZZ_AUTH_TAG=tag, BUZZ_AUTH_TAG=tag)
    relay = _relay_url(extra)
    if relay:
        add.update(KUZZ_RELAY_URL=relay, BUZZ_RELAY_URL=relay)
    me = await _cli_json(cli, ["auth", "whoami"], kuzz_cli.child_env(add=add))
    self_pubkey = normalize_pubkey(me.get("pubkey")) if isinstance(me, dict) else None
    return Identity("legacy", add, self_pubkey or "", configured_owner or _auth_tag_owner(tag))


class _Bounded(OrderedDict):
    """Insertion-ordered map that forgets its oldest entries past ``cap``."""

    def __init__(self, cap: int, items: Iterable[str] = ()) -> None:
        super().__init__()
        self._cap = cap
        for item in items:
            self.put(item)

    def put(self, key: str, value: Any = None) -> None:
        self[key] = value
        self.move_to_end(key)
        while len(self) > self._cap:
            self.popitem(last=False)


# ── Adapter ─────────────────────────────────────────────────────────────────


class BuzzAdapter(BasePlatformAdapter):
    """Hermes ``buzz`` platform over the Kuzz CLI."""

    def __init__(self, config, **kwargs) -> None:
        super().__init__(config=config, platform=Platform(PLATFORM_NAME))
        extra = getattr(config, "extra", None)
        # Keep the config's own dict: the gateway's allowlist check reads it from the live adapter.
        self._extra: dict = extra if isinstance(extra, dict) else {}
        self.relay_url = _relay_url(self._extra)
        self.channels: Set[str] = set(_csv(_setting(("BUZZ_CHANNELS",), self._extra, "channels", [])))
        self.home_channel = str(_setting(("BUZZ_HOME_CHANNEL",), self._extra, "home_channel", "") or "").strip()
        self.require_mention = _flag(_setting(("KUZZ_REQUIRE_MENTION", "BUZZ_REQUIRE_MENTION"), self._extra, "require_mention"), True)
        # Read by Hermes core (gateway/run_turn.py) for buzz progress threading.
        self._reply_to_mode = _reply_to_mode(config, self._extra)
        self._allowed_extra = _pubkeys(
            _csv(_setting(("BUZZ_ALLOWED_USERS",), self._extra, "allowed_users", []))
            + _csv(_setting(("KUZZ_ALLOWED_USERS",), {}, "", []))
        )
        self._reaction_only = _pubkeys(_csv(_setting(("BUZZ_REACTION_ONLY_USERS",), self._extra, "reaction_only_users", [])))
        self._identity: Optional[Identity] = None
        self._cli: List[str] = []
        self._display_name = ""
        self._supervisor: Optional[WatchSupervisor] = None
        self._watch_task: Optional[asyncio.Task] = None
        self._lock_key: Optional[str] = None
        self._connected_at = 0.0
        # channel -> {"chat_type", "last_ts", "seen": _Bounded, "cutoff"}; persisted in the cursor file.
        self._channel_state: Dict[str, dict] = {}
        self._restored: Dict[str, dict] = {}
        self._cursor_saved_at = 0.0
        self._sent = _Bounded(_ID_CAP)
        # event id -> thread root (None for top level), so replies join the thread instead of nesting.
        self._roots = _Bounded(_ID_CAP)
        # Thread roots this agent posted in (or that are its own messages).
        self._our_threads = _Bounded(_ID_CAP)
        self._dm_channels: Set[str] = set()
        self._dm_checked_at = 0.0
        self._channel_names: Dict[str, str] = {}
        self._sync_allowlist()

    @property
    def name(self) -> str:
        return "Buzz"

    @staticmethod
    def normalize_user_id(user_id: str) -> Optional[str]:
        """Allowlist hook for ``gateway/authz_mixin``: npub or hex -> hex."""
        return normalize_pubkey(user_id)

    @property
    def owner(self) -> Optional[str]:
        return self._identity.owner if self._identity else None

    @property
    def allowed_pubkeys(self) -> Set[str]:
        return ({self.owner} if self.owner else set()) | self._allowed_extra

    def _sync_allowlist(self) -> None:
        """Mirror the allowlist into ``config.extra`` so the gateway's own check agrees with ours."""
        allowed = sorted(self.allowed_pubkeys)
        self._extra["allow_from"] = list(allowed)
        self._extra["group_allow_from"] = list(allowed)

    def _env(self) -> Dict[str, str]:
        return kuzz_cli.child_env(add=self._identity.env_add if self._identity else None)

    async def _run(self, args: List[str], input_text: Optional[str] = None) -> Tuple[int, str, str]:
        try:
            return await kuzz_cli.run_cli(self._cli, args, input_text=input_text, env=self._env())
        except OSError as exc:
            return 4, "", kuzz_cli.redact(str(exc))

    # ── Lifecycle ────────────────────────────────────────────────────────

    async def connect(self, *, is_reconnect: bool = False) -> bool:
        if self._supervisor is not None:
            await self.disconnect()
        cli_path = _cli_path(self._extra)
        if not cli_path:
            return self._fail("cli_missing", "The kuzz CLI was not found. Install Kuzz or set KUZZ_CLI_PATH.")
        self._cli = [cli_path]
        identity = await resolve_identity(self._cli, self._extra)
        if identity is None:
            return self._fail("not_paired", "This computer is not paired with Kuzz. Run the `kuzz pair` command from the app (Connect external agent).")
        if identity.self_pubkey and identity.self_pubkey == identity.owner:
            return self._fail(
                "wrong_identity", "The kuzz CLI acts as the owner, not as an agent. Pair the agent with the `kuzz pair` command from the app (Connect external agent)."
            )
        self._identity = identity
        if not self.allowed_pubkeys:
            return self._fail("owner_missing", "No owner or allowed user is known, so nobody could talk to the agent.")
        if identity.mode == "legacy":
            logger.warning(
                "Buzz: this computer is not paired with Kuzz; using the configured key, which Kuzz refuses once "
                "device keys are enforced. Run the `kuzz pair` command from the app (Connect external agent)."
            )
        self._sync_allowlist()
        if not self._acquire_lock():
            return self._fail("lock_conflict", "This agent is already running in another Hermes profile.")
        await self._load_profile()
        self._connected_at = time.time()
        self._load_cursors()
        await self._refresh_dms(force=True)
        self._supervisor = WatchSupervisor(
            self._cli,
            self._on_message,
            on_park=self._on_park,
            on_start=self._on_watch_start,
            env_factory=self._env,
            args_factory=self._watch_args,
        )
        self._watch_task = asyncio.ensure_future(self._supervisor.run())
        self._mark_connected()
        owner = self.owner[:8] + "…" if self.owner else "unknown"
        logger.info("Buzz: watching Kuzz messages (%s mode, owner %s)", identity.mode, owner)
        return True

    async def disconnect(self) -> None:
        self._mark_disconnected()
        supervisor, task = self._supervisor, self._watch_task
        self._supervisor = self._watch_task = None
        if supervisor is not None:
            await supervisor.stop()
        if task is not None and not task.done():
            try:
                await asyncio.wait_for(task, timeout=10)
            except (asyncio.TimeoutError, asyncio.CancelledError):
                task.cancel()
        self._save_cursors()
        self._release_lock()

    def _fail(self, code: str, message: str) -> bool:
        logger.error("Buzz: %s", message)
        self._set_fatal_error(code, message, retryable=False)
        return False

    async def _on_park(self, code: str, message: str) -> None:
        self._save_cursors()
        self._release_lock()
        self._set_fatal_error(code, message, retryable=False)
        await self._notify_fatal_error()

    def _watch_args(self) -> List[str]:
        """``--since`` from the newest message seen (cursor file or this run), so a restart
        catches up on what arrived while Hermes was down. Capped at a day; never in the future."""
        newest = max(
            [s["last_ts"] for s in self._channel_state.values()] + [r["last_ts"] for r in self._restored.values()] + [0]
        )
        if not newest:
            return []
        now = int(time.time())
        since = min(max(int(newest) - _REPLAY_SLACK_SECS, now - _MAX_CATCH_UP_SECS), now)
        return ["--since", str(since)]

    async def _on_watch_start(self) -> None:
        await self._refresh_dms(force=True)

    def _acquire_lock(self) -> bool:
        identity = self._identity.self_pubkey if self._identity and self._identity.self_pubkey else f"cli:{self._cli[0]}"
        try:
            from gateway.status import acquire_scoped_lock
        except Exception:
            return True  # status module unavailable
        ok, _existing = acquire_scoped_lock(PLATFORM_NAME, identity)
        if ok:
            self._lock_key = identity
        return bool(ok)

    def _release_lock(self) -> None:
        if not self._lock_key:
            return
        try:
            from gateway.status import release_scoped_lock

            release_scoped_lock(PLATFORM_NAME, self._lock_key)
        except Exception:
            pass
        self._lock_key = None

    # ── Metadata (best effort) ──────────────────────────────────────────

    async def _load_profile(self) -> None:
        profiles = await _cli_json(self._cli, ["users", "get"], self._env())
        if isinstance(profiles, list) and profiles and isinstance(profiles[0], dict):
            name = profiles[0].get("display_name") or profiles[0].get("name")
            if isinstance(name, str) and name.strip():
                self._display_name = name.strip()

    async def _refresh_dms(self, *, force: bool = False) -> None:
        now = time.monotonic()
        if not force and now - self._dm_checked_at < _DM_REFRESH_SECS:
            return
        self._dm_checked_at = now
        dms = await _cli_json(self._cli, ["dms", "list"], self._env())
        if isinstance(dms, list):
            self._dm_channels = {str(dm.get("dm_id")) for dm in dms if isinstance(dm, dict) and dm.get("dm_id")}

    async def _is_dm(self, channel: str) -> bool:
        if channel not in self._dm_channels:
            await self._refresh_dms()
        return channel in self._dm_channels

    # ── Cursors ($HERMES_HOME/buzz/channel-cursors.json, bundled format) ─

    @staticmethod
    def _cursor_path() -> Path:
        from hermes_constants import get_hermes_home

        return Path(get_hermes_home()) / _CURSOR_SUBDIR / _CURSOR_FILENAME

    def _cursor_identity(self) -> str:
        return self._identity.self_pubkey if self._identity else ""

    def _load_cursors(self) -> None:
        """Restore cursors written for this identity and relay; anything else is ignored."""
        self._restored = {}
        try:
            data = json.loads(self._cursor_path().read_text(encoding="utf-8"))
        except Exception:
            return
        if not isinstance(data, dict) or not self._cursor_identity():
            return
        if data.get("identity") != self._cursor_identity() or data.get("relay") != self.relay_url:
            return
        for channel, entry in (data.get("channels") or {}).items() if isinstance(data.get("channels"), dict) else ():
            if isinstance(entry, dict):
                try:
                    last_ts = int(entry.get("last_ts") or 0)
                except (TypeError, ValueError):
                    continue
                seen = [str(e) for e in entry.get("seen") or []] if isinstance(entry.get("seen"), list) else []
                self._restored[str(channel)] = {"chat_type": str(entry.get("chat_type") or ""), "last_ts": last_ts, "seen": seen}

    def _channel(self, channel: str) -> dict:
        state = self._channel_state.get(channel)
        if state is None:
            restored = self._restored.pop(channel, None)
            if restored is not None:
                cutoff = restored["last_ts"] - _REPLAY_SLACK_SECS
                state = {"chat_type": restored["chat_type"] or "group", "last_ts": restored["last_ts"]}
                state["seen"] = _Bounded(_SEEN_CAP, restored["seen"])
            else:
                cutoff = self._connected_at - _BACKLOG_GRACE_SECS
                state = {"chat_type": "group", "last_ts": 0, "seen": _Bounded(_SEEN_CAP)}
            state["cutoff"] = cutoff
            self._channel_state[channel] = state
        return state

    def _save_cursors(self) -> None:
        if not self._cursor_identity() or not self._channel_state:
            return
        channels = {
            channel: {"chat_type": s["chat_type"], "last_ts": int(s["last_ts"]), "seen": list(s["seen"])}
            for channel, s in self._channel_state.items()
        }
        for channel, restored in self._restored.items():
            channels.setdefault(channel, restored)
        payload = {"identity": self._cursor_identity(), "relay": self.relay_url, "channels": channels}
        try:
            path = self._cursor_path()
            path.parent.mkdir(parents=True, exist_ok=True)
            tmp = path.with_suffix(".tmp")
            with open(tmp, "w", encoding="utf-8", newline="\n") as fh:
                json.dump(payload, fh, separators=(",", ":"))
            os.replace(tmp, path)
            self._cursor_saved_at = time.monotonic()
        except Exception:
            logger.debug("Buzz: could not save channel cursors", exc_info=True)

    def _maybe_save_cursors(self) -> None:
        if time.monotonic() - self._cursor_saved_at >= _CURSOR_SAVE_SECS:
            self._save_cursors()

    # ── Inbound ─────────────────────────────────────────────────────────

    def _resolve_root(self, anchor: Optional[str]) -> Optional[str]:
        """A message id -> the root of its thread (Kuzz threads are one level deep)."""
        if not anchor:
            return None
        root = self._roots.get(str(anchor), _MISSING)
        return str(anchor) if root is _MISSING or root is None else root

    def _self_pubkey(self) -> str:
        return self._identity.self_pubkey if self._identity else ""

    def _mentions_me(self, msg: WatchMessage) -> bool:
        return bool(self._self_pubkey()) and self._self_pubkey() in msg.mentions

    def _is_addressed(self, msg: WatchMessage, is_dm: bool) -> bool:
        if is_dm or not self.require_mention or self._mentions_me(msg):
            return True
        return bool(msg.thread_root and msg.thread_root in self._our_threads)

    def _strip_mention(self, content: str) -> str:
        text = content.strip()
        if self._display_name:
            pattern = rf"^@{re.escape(self._display_name)}(?=$|[\s,;.!?:])[\s:,]*"
            text = re.sub(pattern, "", text, count=1, flags=re.IGNORECASE)
        return text.strip()

    async def _on_message(self, msg: WatchMessage) -> None:
        if self.channels and msg.channel not in self.channels:
            return
        state = self._channel(msg.channel)
        if msg.id in state["seen"]:
            return
        state["seen"].put(msg.id)
        state["last_ts"] = max(state["last_ts"], msg.created_at)
        self._roots.put(msg.id, msg.thread_root)
        try:
            await self._handle(msg, state)
        finally:
            self._maybe_save_cursors()

    async def _handle(self, msg: WatchMessage, state: dict) -> None:
        if msg.author == self._self_pubkey() or msg.id in self._sent:
            self._our_threads.put(msg.thread_root or msg.id)
            return
        if msg.created_at < state["cutoff"]:
            return
        is_dm = await self._is_dm(msg.channel)
        state["chat_type"] = "dm" if is_dm else "group"
        if msg.author not in self.allowed_pubkeys:
            if msg.author in self._reaction_only and self._mentions_me(msg):
                await self.send_reaction(msg.channel, msg.id, _SEEN_REACTION)
            logger.debug("Buzz: ignored a message from a sender who is not allowed (%s…)", msg.author[:8])
            return
        if not self._is_addressed(msg, is_dm) or not self._message_handler:
            return
        user_name = msg.author_name or msg.author[:8]
        source = self.build_source(
            chat_id=msg.channel,
            chat_name=self._channel_names.get(msg.channel, msg.channel),
            chat_type=state["chat_type"],
            user_id=msg.author,
            user_name=user_name,
            thread_id=msg.thread_root,
            message_id=msg.id,
        )
        event = MessageEvent(
            text=self._strip_mention(msg.content),
            message_type=MessageType.TEXT,
            user_id=msg.author,
            user_name=user_name,
            source=source,
            raw_message={
                "id": msg.id,
                "channel": msg.channel,
                "author": msg.author,
                "created_at": msg.created_at,
                "thread_root": msg.thread_root,
            },
            message_id=msg.id,
            timestamp=datetime.fromtimestamp(msg.created_at) if msg.created_at else datetime.now(),
        )
        await self.handle_message(event)
        # "Seen" reaction, as the bundled plugin does.
        await self.send_reaction(msg.channel, msg.id, _SEEN_REACTION)

    # ── Outbound ────────────────────────────────────────────────────────

    def _thread_for(self, chat_id: str, reply_to: Optional[str], metadata: Optional[Dict[str, Any]]) -> Optional[str]:
        """Thread root for a reply. An explicit thread is kept; otherwise channel replies open a
        thread under the message they answer and DM replies stay flat."""
        if self._reply_to_mode == "off":
            return None
        meta = metadata or {}
        if meta.get("thread_id"):
            return self._resolve_root(meta["thread_id"])
        if str(chat_id) in self._dm_channels:
            return None
        return self._resolve_root(meta.get("reply_to_message_id") or reply_to)

    def _record_sent(self, chat_id: str, event_id: str, thread: Optional[str]) -> None:
        self._sent.put(event_id)
        self._roots.put(event_id, thread)
        self._our_threads.put(thread or event_id)
        self._channel(str(chat_id))["seen"].put(event_id)

    async def _post(
        self, chat_id: str, content: str, reply_to: Optional[str], metadata: Optional[Dict[str, Any]], files: Sequence[str] = ()
    ) -> SendResult:
        if not self._cli:
            return SendResult(success=False, error="Buzz is not connected", retryable=True)
        thread = self._thread_for(chat_id, reply_to, metadata)
        outcome = await kuzz_cli.send_message(self._cli, str(chat_id), content, thread=thread, files=files, env=self._env())
        if outcome.event_id is None:
            return SendResult(success=False, error=outcome.error, retryable=outcome.retryable)
        self._record_sent(chat_id, outcome.event_id, thread)
        return SendResult(success=True, message_id=outcome.event_id)

    async def send(
        self, chat_id: str, content: str, reply_to: Optional[str] = None, metadata: Optional[Dict[str, Any]] = None
    ) -> SendResult:
        if not content or not content.strip():
            return SendResult(success=False, error="Empty message")
        return await self._post(chat_id, content, reply_to, metadata)

    async def _send_file(
        self, chat_id: str, path: Any, caption: Optional[str], reply_to: Optional[str], metadata: Optional[Dict[str, Any]]
    ) -> SendResult:
        local = Path(str(path)).expanduser()
        if not local.is_file():
            return SendResult(success=False, error="Media file not found")  # never echo host paths into chat
        return await self._post(chat_id, caption or "", reply_to, metadata, files=[str(local)])

    async def send_image(self, chat_id, image_url, caption=None, reply_to=None, metadata=None) -> SendResult:
        if not str(image_url).startswith(("http://", "https://")) and Path(str(image_url)).expanduser().is_file():
            return await self._send_file(chat_id, image_url, caption, reply_to, metadata)
        return await self.send(chat_id, f"{caption}\n{image_url}" if caption else str(image_url), reply_to, metadata)

    async def send_image_file(self, chat_id, image_path, caption=None, reply_to=None, metadata=None, **kwargs) -> SendResult:
        return await self._send_file(chat_id, image_path, caption, reply_to, metadata)

    async def send_document(
        self, chat_id, file_path, caption=None, file_name=None, reply_to=None, metadata=None, **kwargs
    ) -> SendResult:
        return await self._send_file(chat_id, file_path, caption, reply_to, metadata)

    async def send_video(self, chat_id, video_path, caption=None, reply_to=None, metadata=None, **kwargs) -> SendResult:
        return await self._send_file(chat_id, video_path, caption, reply_to, metadata)

    async def send_voice(self, chat_id, audio_path, caption=None, reply_to=None, metadata=None, **kwargs) -> SendResult:
        return await self._send_file(chat_id, audio_path, caption, reply_to, metadata)

    async def edit_message(self, chat_id: str, message_id: str, content: str, *, finalize: bool = False) -> SendResult:
        """Edit a sent message. Keeps returning the original id, which the stream consumer addresses."""
        if not message_id or not content:
            return SendResult(success=False, error="Edit needs a message id and text")
        if len(content) > _EDIT_ARG_LIMIT:
            return SendResult(success=False, error="Message too long to edit")
        rc, out, err = await self._run(["messages", "edit", "--event", str(message_id), f"--content={content}"])
        if rc != 0:
            return SendResult(success=False, error=kuzz_cli.cli_error_message(err, rc), retryable=rc == 2)
        data = _json_dict(out)
        if data.get("event_id"):
            self._channel(str(chat_id))["seen"].put(str(data["event_id"]))
        return SendResult(success=bool(data.get("accepted", True)), message_id=str(message_id), raw_response=data)

    async def delete_message(self, chat_id: str, message_id: str) -> bool:
        if not message_id:
            return False
        rc, out, _err = await self._run(["messages", "delete", "--event", str(message_id)])
        return rc == 0 and bool(_json_dict(out).get("accepted", True))

    async def send_reaction(self, chat_id: str, message_id: str, emoji: str) -> bool:
        """Best effort; failures are logged at debug level, never raised."""
        if not self._cli or not message_id or not emoji:
            return False
        rc, _out, err = await self._run(["reactions", "add", "--event", str(message_id), "--emoji", emoji])
        if rc != 0:
            logger.debug("Buzz: reaction failed for %s: %s", str(message_id)[:12], kuzz_cli.cli_error_message(err, rc))
        return rc == 0

    async def send_typing(self, chat_id: str, metadata=None) -> None:
        """Kuzz has no typing indicator for agents."""

    async def get_chat_info(self, chat_id: str) -> Dict[str, Any]:
        chat_id = str(chat_id)
        if chat_id not in self._channel_names and self._cli:
            data = await _cli_json(self._cli, ["channels", "get", "--channel", chat_id], self._env())
            if isinstance(data, dict) and data.get("name"):
                self._channel_names[chat_id] = str(data["name"])
        is_dm = chat_id in self._dm_channels
        return {"name": self._channel_names.get(chat_id, chat_id), "type": "dm" if is_dm else "group", "chat_id": chat_id}


def _json_dict(text: str) -> dict:
    try:
        data = json.loads(text or "{}")
    except ValueError:
        return {}
    return data if isinstance(data, dict) else {}


# ── Registration ─────────────────────────────────────────────────────────────


def check_requirements() -> bool:
    """Passive probe: is a buzz/kuzz CLI installed?"""
    return bool(_cli_path({}))


def validate_config(config) -> bool:
    return bool(_cli_path(getattr(config, "extra", None) or {}))


def _env_enablement() -> Optional[dict]:
    """Enable from env (the bundled plugin's variables, or the owner written by `kuzz pair`)."""
    if _profile_scoped() or not check_requirements():
        return None
    relay = os.getenv("BUZZ_RELAY_URL", "").strip()
    owner = normalize_pubkey(os.getenv("KUZZ_OWNER_PUBKEY", ""))
    if not (relay or owner):
        return None
    seed: dict = {"relay_url": relay} if relay else {"owner": owner}
    channels = _csv(os.getenv("BUZZ_CHANNELS", ""))
    if channels:
        seed["channels"] = channels
    home = os.getenv("BUZZ_HOME_CHANNEL", "").strip() or (channels[0] if channels else "")
    if home:
        seed["home_channel"] = {"chat_id": home, "name": os.getenv("BUZZ_HOME_CHANNEL_NAME", home)}
    return seed


async def _standalone_send(
    pconfig, chat_id: str, message: str, *, thread_id: Optional[str] = None,
    media_files: Optional[List[Any]] = None, force_document: bool = False,
) -> Dict[str, Any]:
    """One-shot send without a live adapter (out-of-process ``deliver=buzz`` cron)."""
    extra = getattr(pconfig, "extra", None) or {}
    cli_path = _cli_path(extra)
    if not cli_path:
        return {"error": "Buzz standalone send: kuzz CLI not found"}
    identity = await resolve_identity([cli_path], extra)
    if identity is None:
        return {"error": "Buzz standalone send: this computer is not paired with Kuzz"}
    home = extra.get("home_channel")
    home = home.get("chat_id") if isinstance(home, dict) else home
    target = (chat_id or "").strip() or str(_setting(("BUZZ_HOME_CHANNEL",), {}, "", "") or home or "").strip()
    if not target:
        return {"error": "Buzz standalone send: no target channel (set BUZZ_HOME_CHANNEL)"}
    thread = thread_id if thread_id and _reply_to_mode(pconfig, extra) != "off" else None
    files = [str(m[0] if isinstance(m, (list, tuple)) and m else m) for m in media_files or []]
    outcome = await kuzz_cli.send_message(
        [cli_path], target, message, thread=thread, files=files, env=kuzz_cli.child_env(add=identity.env_add)
    )
    if outcome.event_id is None:
        return {"error": f"Buzz standalone send failed: {outcome.error}"}
    result: Dict[str, Any] = {"success": True, "message_id": outcome.event_id}
    if files:
        result["media_delivered"] = True
    return result


def register(ctx) -> None:
    ctx.register_platform(
        name=PLATFORM_NAME,
        label="Buzz",
        adapter_factory=lambda cfg: BuzzAdapter(cfg),
        check_fn=check_requirements,
        validate_config=validate_config,
        is_connected=validate_config,
        required_env=[],
        install_hint="Install Kuzz, then pair this computer with the `kuzz pair` command from the app (Connect external agent).",
        env_enablement_fn=_env_enablement,
        cron_deliver_env_var="BUZZ_HOME_CHANNEL",
        standalone_sender_fn=_standalone_send,
        emoji="\U0001f41d",
        pii_safe=False,  # identities are pubkeys, not phone numbers
        allow_update_command=True,
        platform_hint=(
            "You are collaborating in a Kuzz workspace (a human+agent messenger). Markdown IS "
            "supported. Users address you by @-mentioning your name in channels or by replying "
            "in your thread; direct messages reach you without a mention. Keep responses conversational."
        ),
    )
