"""Transport for the Hermes Kuzz plugin: the ``kuzz`` CLI, nothing else.

Receiving is ``kuzz messages watch`` (one JSON object per line). Sending is
``kuzz messages send --channel <uuid> [--reply-in-thread <id>] --content -``
with the text on stdin. A paired CLI signs with the identity bound to this
computer (``kuzz pair``). This module never reads or logs a key or token; it
starts every child from :func:`child_env`, which removes them and hides any
person's ``kuzz auth login``, so the CLI acts as the paired agent.

No Hermes imports here, so the parsing, send and supervision paths can be
unit-tested with a fake ``kuzz`` binary.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import shutil
import tempfile
import time
from collections import deque
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Awaitable, Callable, Dict, Mapping, Optional, Sequence, Tuple

logger = logging.getLogger(__name__)

# Removed from the environment of every ``kuzz`` child process:
# - allow-all flags, so no inherited setting can open the agent to everyone;
# - raw credentials, so the CLI always speaks as the identity paired to this
#   computer (a raw key or token is refused once device admission is enforced).
STRIPPED_ENV = (
    "KUZZ_ALLOW_ALL_USERS",
    "BUZZ_ALLOW_ALL_USERS",
    "KUZZ_PRIVATE_KEY",
    "BUZZ_PRIVATE_KEY",
    "KUZZ_AUTH_TAG",
    "BUZZ_AUTH_TAG",
    "KUZZ_BOT_TOKEN",
    "KUZZ_ACCESS_TOKEN",
    "KUZZ_TOKEN_BROKER_URL",
    "KUZZ_TOKEN_BROKER_SECRET",
    "KUZZ_SESSION_FILE",
)

# A person's stored login (`kuzz auth login`) outranks the paired agent for its
# relay. Children get a session file that never exists, so they act as the agent.
NO_LOGIN_SESSION_FILE = str(Path(tempfile.gettempdir()) / "kuzz-hermes-no-login" / "session.json")

CLI_TIMEOUT = 30.0
# Largest accepted watch line; a longer line restarts the watch.
WATCH_LINE_LIMIT = 4 * 1024 * 1024
_MAX_DETAIL_CHARS = 500

# Exit details that mean "this computer is no longer allowed to act for the
# agent" or "this CLI cannot watch": restarting cannot help, so the watch parks.
_PARK_MARKERS = (
    ("signed_out", "Kuzz signed this agent out. Pair it again with `kuzz pair`."),
    ("revoked", "The owner disconnected this agent. Pair it again with `kuzz pair`."),
    ("not_paired", "This computer is not paired. Run the `kuzz pair` command from the app (Connect external agent) first."),
    ("unrecognized subcommand", "This kuzz CLI has no `messages watch`. Update Kuzz."),
)

_HEX64 = re.compile(r"^[0-9a-f]{64}$")
_SECRET_PATTERNS = (
    (re.compile(r"nsec1[0-9a-z]{8,}", re.IGNORECASE), "nsec1[redacted]"),
    (re.compile(r"(Bearer\s+)\S+", re.IGNORECASE), r"\1[redacted]"),
    (re.compile(r"(kuzz pair\s+)\S+", re.IGNORECASE), r"\1[redacted]"),
)

# ── Identities ───────────────────────────────────────────────────────────────

_BECH32 = "qpzry9x8gf2tvdw0s3jn54khce6mua7l"
_BECH32_GEN = (0x3B6A57B2, 0x26508E6D, 0x1EA119FA, 0x3D4233DD, 0x2A1462B3)


def _bech32_polymod(values) -> int:
    chk = 1
    for value in values:
        top = chk >> 25
        chk = ((chk & 0x1FFFFFF) << 5) ^ value
        for i, gen in enumerate(_BECH32_GEN):
            if (top >> i) & 1:
                chk ^= gen
    return chk


def _npub_to_hex(ref: str) -> Optional[str]:
    if not ref.startswith("npub1"):
        return None
    data = ref[5:]
    if len(data) < 7 or any(c not in _BECH32 for c in data):
        return None
    values = [_BECH32.index(c) for c in data]
    hrp = [ord(c) >> 5 for c in "npub"] + [0] + [ord(c) & 31 for c in "npub"]
    if _bech32_polymod(hrp + values) != 1:
        return None
    acc = bits = 0
    out = bytearray()
    for value in values[:-6]:
        acc = (acc << 5) | value
        bits += 5
        while bits >= 8:
            bits -= 8
            out.append((acc >> bits) & 0xFF)
    if bits >= 5 or acc & ((1 << bits) - 1):
        return None
    return out.hex() if len(out) == 32 else None


def normalize_pubkey(ref: Any) -> Optional[str]:
    """Hex or npub -> lowercase hex pubkey; anything else -> None."""
    if not isinstance(ref, str):
        return None
    text = ref.strip().lower()
    if _HEX64.fullmatch(text):
        return text
    return _npub_to_hex(text)


# ── Process plumbing ─────────────────────────────────────────────────────────


def resolve_cli_path(*configured: str) -> str:
    """The CLI binary. The first non-empty configured path decides alone (missing -> "").
    Otherwise: ``buzz`` on PATH, ``~/bin/buzz`` (the bundled plugin's lookup), then
    ``kuzz`` on PATH and the Kuzz desktop app's copy."""
    for path in configured:
        if path and path.strip():
            path = path.strip()
            return shutil.which(path) or (path if Path(path).expanduser().is_file() else "")
    for candidate in (shutil.which("buzz"), str(Path.home() / "bin" / "buzz")):
        if candidate and Path(candidate).is_file():
            return candidate
    return find_kuzz()


def find_kuzz() -> str:
    """``kuzz`` on PATH, then the Kuzz desktop app's copy; "" if neither exists."""
    candidates = [shutil.which("kuzz")]
    local = os.environ.get("LOCALAPPDATA")
    if local:
        candidates.append(str(Path(local) / "KuzzApp" / "kuzz.exe"))
    for candidate in candidates:
        if candidate and Path(candidate).is_file():
            return candidate
    return ""


def child_env(base: Optional[Mapping[str, str]] = None, add: Optional[Mapping[str, str]] = None) -> Dict[str, str]:
    """Environment for a ``kuzz`` child: the parent's, minus :data:`STRIPPED_ENV`, with no
    stored login (:data:`NO_LOGIN_SESSION_FILE`), plus ``add``."""
    env = dict(os.environ if base is None else base)
    for name in STRIPPED_ENV:
        env.pop(name, None)
    env["KUZZ_SESSION_FILE"] = NO_LOGIN_SESSION_FILE
    env.update(add or {})
    return env


def redact(text: str) -> str:
    """Bound CLI output for logs and strip anything shaped like a secret."""
    for pattern, replacement in _SECRET_PATTERNS:
        text = pattern.sub(replacement, text)
    text = " ".join(text.split())
    if len(text) > _MAX_DETAIL_CHARS:
        text = text[: _MAX_DETAIL_CHARS - 3] + "..."
    return text


async def run_cli(
    cli: Sequence[str],
    args: Sequence[str],
    *,
    input_text: Optional[str] = None,
    timeout: float = CLI_TIMEOUT,
    env: Optional[Mapping[str, str]] = None,
) -> Tuple[int, str, str]:
    """Run ``kuzz`` (argv, never a shell) -> ``(rc, stdout, stderr)``."""
    proc = await asyncio.create_subprocess_exec(
        *cli,
        *args,
        stdin=asyncio.subprocess.PIPE if input_text is not None else asyncio.subprocess.DEVNULL,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        env=dict(env) if env is not None else child_env(),
    )
    stdin_bytes = input_text.encode("utf-8") if input_text is not None else None
    try:
        out, err = await asyncio.wait_for(proc.communicate(stdin_bytes), timeout=timeout)
    except asyncio.TimeoutError:
        proc.kill()
        await proc.wait()
        return 124, "", json.dumps({"error": "timeout", "message": f"kuzz timed out after {timeout:.0f}s"})
    rc = proc.returncode if proc.returncode is not None else 4
    return rc, out.decode("utf-8", errors="replace"), err.decode("utf-8", errors="replace")


def cli_error_message(stderr: str, rc: int) -> str:
    """Readable, bounded message from the CLI error contract (JSON on stderr)."""
    text = (stderr or "").strip()
    try:
        data = json.loads(text)
    except ValueError:
        data = None
    if isinstance(data, dict) and isinstance(data.get("message"), str) and data["message"].strip():
        label = data.get("error") if isinstance(data.get("error"), str) else "error"
        return redact(f"{label}: {data['message'].strip()} (exit {rc})")
    return redact(text) or f"kuzz exited with code {rc}"


# ── Sending ──────────────────────────────────────────────────────────────────


def parse_send_receipt(stdout: str) -> Tuple[Optional[str], Optional[str]]:
    """``{"event_id", "accepted", "message"}`` -> ``(event_id, error)``."""
    try:
        data = json.loads(stdout)
    except ValueError:
        return None, "invalid kuzz response"
    if not isinstance(data, dict):
        return None, "invalid kuzz response"
    if data.get("accepted") is False:
        detail = data.get("message")
        return None, redact(detail) if isinstance(detail, str) and detail.strip() else "message was not accepted"
    event_id = data.get("event_id")
    if data.get("accepted") is not True or not isinstance(event_id, str) or not event_id.strip():
        return None, "invalid kuzz response"
    return event_id.strip(), None


def send_args(channel: str, thread: Optional[str] = None, files: Sequence[str] = ()) -> list:
    args = ["messages", "send", "--channel", str(channel)]
    if thread:
        args += ["--reply-in-thread", str(thread)]
    for path in files:
        args += ["--file", str(path)]
    return args + ["--content", "-"]


@dataclass(frozen=True)
class SendOutcome:
    event_id: Optional[str]
    error: Optional[str] = None
    retryable: bool = False


async def send_message(
    cli: Sequence[str],
    channel: str,
    content: str,
    *,
    thread: Optional[str] = None,
    files: Sequence[str] = (),
    env: Optional[Mapping[str, str]] = None,
    timeout: float = CLI_TIMEOUT,
) -> SendOutcome:
    """Post ``content`` (through stdin) to ``channel``, optionally inside a thread or with files."""
    args = send_args(channel, thread, files)
    try:
        rc, out, err = await run_cli(cli, args, input_text=content, env=env, timeout=timeout)
    except OSError as exc:
        return SendOutcome(None, f"could not start kuzz: {redact(str(exc))}")
    if rc != 0:
        error = cli_error_message(err, rc)
        for path in files:
            error = error.replace(str(path), Path(path).name)  # never echo host paths into chat
        # 2 = network / delivery unknown, 124 = timeout: worth a retry.
        return SendOutcome(None, error, retryable=rc in (2, 124))
    event_id, error = parse_send_receipt(out)
    return SendOutcome(event_id, error)


# ── Receiving ────────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class WatchMessage:
    """One ``kuzz messages watch`` line."""

    id: str
    channel: str
    author: str
    author_name: Optional[str]
    content: str
    created_at: int
    thread_root: Optional[str]
    mentions: Tuple[str, ...]


def parse_watch_line(line: Any) -> Optional[WatchMessage]:
    """Validate one watch line; None when it is blank or not a message. Extra fields are ignored."""
    if isinstance(line, (bytes, bytearray)):
        line = bytes(line).decode("utf-8", errors="replace")
    if not isinstance(line, str) or not line.strip():
        return None
    try:
        data = json.loads(line)
    except ValueError:
        return None
    if not isinstance(data, dict):
        return None
    event_id, channel, content = data.get("id"), data.get("channel"), data.get("content")
    author = normalize_pubkey(data.get("author"))
    if not (isinstance(event_id, str) and event_id.strip() and isinstance(channel, str) and channel.strip()):
        return None
    if author is None or not isinstance(content, str):
        return None
    created_at = data.get("created_at")
    if isinstance(created_at, bool) or not isinstance(created_at, (int, float)):
        return None
    root = data.get("thread_root")
    root = root.strip() if isinstance(root, str) and root.strip() else None
    name = data.get("author_name")
    name = name.strip() if isinstance(name, str) and name.strip() else None
    raw_mentions = data.get("mentions")
    mentions = tuple(
        pk for pk in (normalize_pubkey(m) for m in (raw_mentions if isinstance(raw_mentions, list) else [])) if pk
    )
    return WatchMessage(
        id=event_id.strip(),
        channel=channel.strip(),
        author=author,
        author_name=name,
        content=content,
        created_at=int(created_at),
        thread_root=root,
        mentions=mentions,
    )


def classify_exit(rc: Optional[int], detail: str) -> Optional[Tuple[str, str]]:
    """``(code, message)`` when the watch must park instead of restarting, else None."""
    lowered = detail.lower()
    for marker, message in _PARK_MARKERS:
        if marker in lowered:
            return marker.replace(" ", "_"), message
    return None


OnMessage = Callable[[WatchMessage], Awaitable[None]]
OnPark = Callable[[str, str], Awaitable[None]]
OnStart = Callable[[], Awaitable[None]]


class WatchSupervisor:
    """Keep one ``kuzz messages watch`` running: restart with backoff, park on fatal exits."""

    def __init__(
        self,
        cli: Sequence[str],
        on_message: OnMessage,
        *,
        on_park: Optional[OnPark] = None,
        on_start: Optional[OnStart] = None,
        env_factory: Callable[[], Mapping[str, str]] = child_env,
        args_factory: Callable[[], Sequence[str]] = tuple,
        min_backoff: float = 1.0,
        max_backoff: float = 60.0,
        healthy_after: float = 60.0,
        stop_timeout: float = 5.0,
    ) -> None:
        self._cli = list(cli)
        self._on_message = on_message
        self._on_park = on_park
        self._on_start = on_start
        self._env_factory = env_factory
        self._args_factory = args_factory
        self._min_backoff = min_backoff
        self._max_backoff = max_backoff
        self._healthy_after = healthy_after
        self._stop_timeout = stop_timeout
        self._stop = asyncio.Event()
        self._proc: Optional[asyncio.subprocess.Process] = None
        self.restarts = 0
        self.parked: Optional[Tuple[str, str]] = None

    @property
    def stopping(self) -> bool:
        return self._stop.is_set()

    async def run(self) -> None:
        backoff = self._min_backoff
        while not self._stop.is_set():
            started = time.monotonic()
            try:
                rc, detail = await self._run_once()
            except OSError as exc:
                rc, detail = None, f"could not start kuzz: {exc}"
            if self._stop.is_set():
                return
            detail = redact(detail)
            park = classify_exit(rc, detail)
            if park is not None:
                self.parked = park
                logger.error("Kuzz: watch stopped for good (%s): %s", park[0], detail or park[1])
                if self._on_park is not None:
                    await self._on_park(*park)
                return
            if time.monotonic() - started >= self._healthy_after:
                backoff = self._min_backoff
            self.restarts += 1
            logger.warning("Kuzz: watch exited (rc=%s); restarting in %.0fs. %s", rc, backoff, detail)
            try:
                await asyncio.wait_for(self._stop.wait(), timeout=backoff)
                return
            except asyncio.TimeoutError:
                pass
            backoff = min(backoff * 2, self._max_backoff)

    async def _run_once(self) -> Tuple[Optional[int], str]:
        proc = await asyncio.create_subprocess_exec(
            *self._cli,
            "messages",
            "watch",
            *self._args_factory(),
            stdin=asyncio.subprocess.DEVNULL,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            env=dict(self._env_factory()),
            limit=WATCH_LINE_LIMIT,
        )
        self._proc = proc
        tail: deque = deque(maxlen=20)
        drain = asyncio.ensure_future(self._drain(proc.stderr, tail))
        try:
            if self._on_start is not None:
                try:
                    await self._on_start()
                except Exception:
                    logger.exception("Kuzz: watch start hook failed")
            while not self._stop.is_set():
                try:
                    line = await proc.stdout.readline()
                except ValueError:
                    tail.append(f"watch line longer than {WATCH_LINE_LIMIT} bytes")
                    break
                if not line:
                    break
                message = parse_watch_line(line)
                if message is None:
                    if line.strip():
                        logger.debug("Kuzz: ignored a watch line that is not a message")
                    continue
                try:
                    await self._on_message(message)
                except Exception:
                    logger.exception("Kuzz: message handler failed for %s", message.id[:12])
        finally:
            if not self._stop.is_set():
                # Output ended: let the CLI exit by itself so its exit code and stderr are real.
                try:
                    await asyncio.wait_for(proc.wait(), timeout=self._stop_timeout)
                except asyncio.TimeoutError:
                    pass
            await self._terminate(proc)
            self._proc = None
            try:
                await asyncio.wait_for(drain, timeout=self._stop_timeout)
            except (asyncio.TimeoutError, asyncio.CancelledError):
                drain.cancel()
        return proc.returncode, " ".join(tail)

    @staticmethod
    async def _drain(stream: Optional[asyncio.StreamReader], tail: deque) -> None:
        if stream is None:
            return
        while True:
            line = await stream.readline()
            if not line:
                return
            text = line.decode("utf-8", errors="replace").strip()
            if text:
                tail.append(text)

    async def _terminate(self, proc: asyncio.subprocess.Process) -> None:
        if proc.returncode is not None:
            return
        try:
            proc.terminate()
        except ProcessLookupError:
            return
        try:
            await asyncio.wait_for(proc.wait(), timeout=self._stop_timeout)
        except asyncio.TimeoutError:
            try:
                proc.kill()
            except ProcessLookupError:
                return
            await proc.wait()

    async def stop(self) -> None:
        """Stop for good: end the running watch and the restart loop."""
        self._stop.set()
        proc = self._proc
        if proc is not None:
            await self._terminate(proc)
