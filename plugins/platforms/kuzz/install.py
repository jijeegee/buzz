"""Install the Kuzz platform plugin into a Hermes home.

    python install.py [--owner PUBKEY] [--hermes-home DIR] [--cli-path PATH] [--no-enable]
    python install.py --refresh [--hermes-home DIR] [--no-restart]

What it does (the same steps `kuzz pair --hermes` and the desktop Hermes card follow):
1. copies PLUGIN_FILES into <HERMES_HOME>/plugins/kuzz/;
2. sets KUZZ_CLI_PATH (the given path, else the installed kuzz) and, when given,
   KUZZ_OWNER_PUBKEY in <HERMES_HOME>/.env;
3. adds ``kuzz-platform`` to ``plugins.enabled`` (and drops it from ``plugins.disabled``) with
   `hermes config get` / `hermes config set`, then retires an earlier Kuzz build that took
   Hermes' ``buzz`` platform (a real Buzz plugin is left alone);
4. with ``--agent``, pins KUZZ_PAIRED_AGENT in .env, so this home keeps acting as that agent
   when more pairings exist on the computer;
5. with an owner and no home channel yet, sets KUZZ_HOME_CHANNEL to the owner's DM (opened by
   the paired agent), so cron results reach Kuzz and Hermes does not ask for /sethome;
6. restarts the Hermes gateway (``--no-restart`` skips it) so the plugin loads now. A stopped
   gateway is started; a busy one is left alone and a manual restart is asked for.

`kuzz pair --hermes` runs ``python install.py --owner <hex> --agent <hex> --hermes-home <dir>``
(plus ``--no-restart`` when the app restarts the gateway itself) from its
install root: the kuzz binary's folder, ``<prefix>/share/kuzz``, ``Contents/Resources`` of the
macOS app (which also passes --cli-path), or ``<local data>/kuzz``, each holding
``plugins/platforms/kuzz/``. The kuzz binary in that root is preferred.
To remove it, delete <HERMES_HOME>/plugins/kuzz/. Stdlib only; no keys are read or written.

``--refresh`` (what `kuzz hermes refresh` and the desktop app run after an update) only replaces
the files of a home that already has the plugin, then restarts a running gateway once it is idle.
Settings, the pairing and the home channel are left alone. It prints one JSON line on stdout.
"""

from __future__ import annotations

import argparse
import filecmp
import json
import os
import shutil
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Dict, List, Optional

HERE = Path(__file__).resolve().parent
PLUGIN_NAME = "kuzz-platform"
INSTALL_DIR = "kuzz"
# An earlier Kuzz build replaced the bundled Buzz plugin under its name and folder.
OLD_PLUGIN_NAME = "buzz-platform"
OLD_INSTALL_DIR = "buzz"
OLD_VERSION_SUFFIX = "-kuzz"
PLUGIN_FILES = ("plugin.yaml", "__init__.py", "adapter.py", "kuzz_cli.py")
HOME_ENV = "KUZZ_HOME_CHANNEL"
PAIRED_AGENT_ENV = "KUZZ_PAIRED_AGENT"
# Step limits. `kuzz pair --hermes` from the app gives claim + install 180 s and restarts the
# gateway itself (--no-restart), so only a terminal run waits for RESTART_TIMEOUT; Hermes drains
# live work for up to 180 s before it stops, and a killed restart can leave no gateway running.
CONFIG_TIMEOUT = 30
DM_TIMEOUT = 20
RESTART_TIMEOUT = 300
HELP_TIMEOUT = 30
# Refresh: the gateway that was running when files were replaced, under <root>/kuzz/. From Hermes's
# state file: start_time drifts between readings, and a heartbeat counts as stale at 120 s.
PENDING_FILE = "refresh-pending.json"
START_TIME_DRIFT = 200
HEARTBEAT_FRESH = 120

sys.path.insert(0, str(HERE))
from kuzz_cli import STRIPPED_ENV, child_env, find_kuzz, is_runnable, normalize_pubkey, which_program  # noqa: E402  (stdlib-only)


def default_hermes_home() -> Path:
    env = os.environ.get("HERMES_HOME", "").strip()
    if env:
        return Path(env)
    local = os.environ.get("LOCALAPPDATA", "").strip()
    if os.name == "nt" and local:
        return Path(local) / "hermes"
    return Path.home() / ".hermes"


def back_up(hermes_home: Path, folder: Path) -> Path:
    """Move a plugin folder to <hermes_home>/plugin-backups/<name>-<UTC time>/ (outside plugins/,
    so Hermes does not load it), never deleted."""
    backups = hermes_home / "plugin-backups"
    backups.mkdir(parents=True, exist_ok=True)
    stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    backup = backups / f"{folder.name}-{stamp}"
    n = 1
    while backup.exists():
        n += 1
        backup = backups / f"{folder.name}-{stamp}-{n}"
    folder.rename(backup)
    print(f"Moved the previous {folder} to {backup}", file=sys.stderr)
    return backup


def has_old_connector(hermes_home: Path) -> bool:
    """An earlier Kuzz build sits in the ``buzz`` slot. Only ours: its manifest version ends in -kuzz."""
    try:
        manifest = (hermes_home / "plugins" / OLD_INSTALL_DIR / "plugin.yaml").read_text(encoding="utf-8")
    except OSError:
        return False
    version = next((line.partition(":")[2].strip() for line in manifest.splitlines() if line.startswith("version:")), "")
    return version.endswith(OLD_VERSION_SUFFIX)


def retire_old_connector(hermes_home: Path) -> None:
    """Take the earlier build out, so it can't answer beside this one and bundled Buzz gets its place back."""
    back_up(hermes_home, hermes_home / "plugins" / OLD_INSTALL_DIR)
    print(
        f"Check {hermes_home / '.env'}: Kuzz no longer reads BUZZ_ settings, and Buzz would. Move BUZZ_CHANNELS, "
        "BUZZ_HOME_CHANNEL, BUZZ_REQUIRE_MENTION and BUZZ_REACTION_ONLY_USERS to their KUZZ_ names, and cron "
        "deliver=buzz to deliver=kuzz. Remove BUZZ_ values that were for Kuzz."
    )


def copy_plugin(hermes_home: Path) -> Path:
    """Replace <hermes_home>/plugins/kuzz with a fresh copy of PLUGIN_FILES."""
    plugins = hermes_home / "plugins"
    target = plugins / INSTALL_DIR
    staging = plugins / f".{INSTALL_DIR}.installing"
    plugins.mkdir(parents=True, exist_ok=True)
    shutil.rmtree(staging, ignore_errors=True)
    staging.mkdir(exist_ok=True)
    for name in PLUGIN_FILES:
        shutil.copy2(HERE / name, staging / name)
    backup = back_up(hermes_home, target) if target.exists() else None
    try:
        staging.rename(target)
    except OSError:
        if backup:
            backup.rename(target)
        raise
    return target


def set_env_values(env_file: Path, values: dict) -> None:
    """Upsert KEY=value lines in a dotenv file. Every line that sets a key is updated (a later
    duplicate would otherwise win); other lines and the file's line endings are kept."""
    text = ""
    if env_file.exists():
        with open(env_file, encoding="utf-8", newline="") as fh:
            text = fh.read()
    newline = "\r\n" if "\r\n" in text else "\n"
    lines = text.splitlines()
    seen = set()
    for i, line in enumerate(lines):
        head, sep, _value = line.partition("=")
        key = head.strip()
        prefix = ""
        if key.startswith("export "):
            prefix, key = "export ", key[len("export "):].strip()
        if sep and key in values:
            lines[i] = f"{prefix}{key}={values[key]}"
            seen.add(key)
    lines += [f"{key}={value}" for key, value in values.items() if key not in seen]
    env_file.parent.mkdir(parents=True, exist_ok=True)
    with open(env_file, "w", encoding="utf-8", newline="") as fh:
        fh.write(newline.join(lines) + newline)


def hermes_root(hermes_home: Path) -> Path:
    """The default profile's home, for a named profile's ``<root>/profiles/<name>``."""
    return hermes_home.parent.parent if hermes_home.parent.name == "profiles" else hermes_home


def find_hermes(hermes_home: Path) -> Optional[str]:
    """The ``hermes`` launcher; on Windows only ``hermes.exe`` (a .cmd/.bat is never run)."""
    found = which_program("hermes")
    if found and is_runnable(found):
        return found
    name = "hermes.exe" if os.name == "nt" else "hermes"
    root = hermes_root(hermes_home)
    for folder in (root / "bin", root / "hermes-agent" / "venv" / "Scripts",
                   root / "hermes-agent" / "venv" / "bin", Path.home() / ".local" / "bin"):
        candidate = folder / name
        if is_runnable(str(candidate)):
            return str(candidate)
    return None


def find_installed_kuzz() -> str:
    """The kuzz binary of the install root this file ships in, else :func:`find_kuzz`."""
    root = HERE.parents[2]  # <root>/plugins/platforms/kuzz/install.py
    candidates = [root / "kuzz.exe", root / "kuzz"]
    if root.name == "kuzz" and root.parent.name == "share":  # <prefix>/share/kuzz -> <prefix>/bin/kuzz
        candidates.append(root.parent.parent / "bin" / "kuzz")
    for candidate in candidates:
        if is_runnable(str(candidate)):
            return str(candidate)
    return find_kuzz()


def hermes_env(hermes_home: Path) -> dict:
    """Env for ``hermes`` children: this one's, pointed at ``hermes_home``, without Kuzz/Buzz
    settings, keys or tokens (a restarted gateway inherits it; Hermes reads them from .env)."""
    env = {k: v for k, v in os.environ.items()
           if k not in STRIPPED_ENV and not k.upper().startswith(("KUZZ_", "BUZZ_"))}
    env["HERMES_HOME"] = str(hermes_home)
    return env


def _hermes_config(hermes: str, env: dict, *args: str) -> subprocess.CompletedProcess:
    """``hermes config <args>``; a timeout or launch failure reads as an error (rc 1)."""
    try:
        return subprocess.run(
            [hermes, "config", *args], env=env, stdin=subprocess.DEVNULL, capture_output=True,
            text=True, encoding="utf-8", errors="replace", timeout=CONFIG_TIMEOUT, check=False,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        return subprocess.CompletedProcess([hermes, "config", *args], 1, "", str(exc))


def _not_set(result: subprocess.CompletedProcess) -> bool:
    return result.returncode != 0 and "not set" in (result.stdout + result.stderr)


def _read_list(hermes: str, env: dict, key: str) -> Optional[List[str]]:
    """``hermes config get <key>`` as a list of strings; [] when unset; None when unreadable."""
    result = _hermes_config(hermes, env, "get", key)
    if result.returncode != 0:
        return [] if _not_set(result) else None
    text = result.stdout.strip()
    if text in ("", "[]", "null", "None"):
        return []
    items = []
    for line in text.splitlines():
        if not line.startswith("- "):
            return None  # not a plain list: leave the config alone
        items.append(line[2:].strip().strip("'\""))
    return items


def enable_plugin(hermes_home: Path, retired: bool = False) -> bool:
    """Add PLUGIN_NAME to ``plugins.enabled`` and drop it from ``plugins.disabled``. ``retired``:
    also drop the old connector's name, which an earlier install added."""
    hermes = find_hermes(hermes_home)
    if not hermes:
        return False
    env = hermes_env(hermes_home)
    enabled = _read_list(hermes, env, "plugins.enabled")
    disabled = _read_list(hermes, env, "plugins.disabled")
    if enabled is None or disabled is None:
        return False
    wanted = [name for name in enabled if not (retired and name == OLD_PLUGIN_NAME)]
    if PLUGIN_NAME not in wanted:
        wanted.append(PLUGIN_NAME)
    if wanted != enabled:
        result = _hermes_config(hermes, env, "set", "plugins.enabled", json.dumps(wanted))
        if result.returncode != 0:
            return False
    if PLUGIN_NAME in disabled:
        remaining = [name for name in disabled if name != PLUGIN_NAME]
        result = _hermes_config(hermes, env, "set", "plugins.disabled", json.dumps(remaining))
        if result.returncode != 0:
            return False
    return True


def read_env_value(env_file: Path, key: str) -> str:
    """The last ``key=value`` in a dotenv file (quotes stripped), else ""."""
    if not env_file.exists():
        return ""
    value = ""
    with open(env_file, encoding="utf-8", errors="replace") as fh:
        for line in fh:
            head, sep, rest = line.partition("=")
            name = head.strip()
            if name.startswith("export "):
                name = name[len("export "):].strip()
            if sep and name == key:
                value = rest.strip().strip("'\"")
    return value


def set_home_channel(
    hermes_home: Path, cli_path: str, owner: str, agent: Optional[str] = None, replace: bool = False,
) -> Optional[str]:
    """Make the owner's DM the Kuzz home channel (cron results and notices) unless one is set.

    The home is written the way ``/sethome`` mirrors it, KUZZ_HOME_CHANNEL in .env, which the
    plugin seeds into the gateway config at start (and which wins over config.yaml). A home already
    in .env or config.yaml is kept, and nothing is written when config.yaml cannot be read (Hermes
    missing or failing). ``replace``: this home now acts as another agent or owner, so the old home
    (a DM the new agent is not in) is replaced. Returns the DM id when one was set."""
    env_file = hermes_home / ".env"
    if not replace:
        if read_env_value(env_file, HOME_ENV):
            return None
        hermes = find_hermes(hermes_home)
        if not hermes or not _not_set(_hermes_config(hermes, hermes_env(hermes_home), "get", "platforms.kuzz.home_channel")):
            return None
    # The paired agent opens (or finds) its DM with the owner; a stored person login is skipped.
    try:
        result = subprocess.run(
            [cli_path, "dms", "open", "--pubkey", owner],
            env=child_env(home=hermes_home, add={PAIRED_AGENT_ENV: agent} if agent else None),
            stdin=subprocess.DEVNULL, capture_output=True, text=True, encoding="utf-8",
            errors="replace", timeout=DM_TIMEOUT, check=False,
        )
        dm_id = json.loads(result.stdout).get("dm_id") if result.returncode == 0 else None
    except (OSError, subprocess.SubprocessError, ValueError, AttributeError):
        dm_id = None
    if not isinstance(dm_id, str) or not dm_id.strip():
        return None
    set_env_values(env_file, {HOME_ENV: dm_id.strip()})
    return dm_id.strip()


def pin_default_profile(hermes_home: Path, default_agents: Dict[str, str]) -> None:
    """Pairing a named profile makes it the newest pairing. Keep the default profile on the agent
    it acted as: its owner's latest other agent, if that install has no pin yet."""
    root = hermes_root(hermes_home)
    if root == hermes_home or read_env_value(root / ".env", PAIRED_AGENT_ENV):
        return
    agent = default_agents.get(normalize_pubkey(read_env_value(root / ".env", "KUZZ_OWNER_PUBKEY") or "") or "")
    if agent:
        set_env_values(root / ".env", {PAIRED_AGENT_ENV: agent})


def restart_profile(hermes_home: Path) -> str:
    """The profile whose gateway runs ``hermes_home``: its own when it ever ran one or has a
    launchd job (one gateway per profile), else the host, which serves the profiles and loads
    plugins. Always named: a sticky ``hermes profile use`` would otherwise bounce another profile.
    The same rule as the app's ``restart_profile`` (desktop ``hermes_gateway.rs``)."""
    root = hermes_root(hermes_home)
    if hermes_home == root:
        return "default"
    name = hermes_home.name
    served = read_json(root / "gateway_state.json").get("served_profiles")
    if isinstance(served, list) and name in served:
        return "default"
    service = Path.home() / "Library" / "LaunchAgents" / f"ai.hermes.gateway-{name}.plist"
    own = read_json(hermes_home / "gateway_state.json")
    return name if "pid" in own or (sys.platform == "darwin" and service.is_file()) else "default"


def supports_detach(hermes: str, env: dict) -> bool:
    """``--detach`` only exists in newer Hermes; an older one exits 2 on it."""
    try:
        result = subprocess.run(
            [hermes, "gateway", "restart", "--help"], env=env, stdin=subprocess.DEVNULL, capture_output=True,
            text=True, encoding="utf-8", errors="replace", timeout=HELP_TIMEOUT, check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return False
    return "--detach" in result.stdout


def await_new_gateway(command: List[str], env: dict, state_file: Path) -> bool:
    """A restart without ``--detach``: without a service it stays on as the gateway, so it runs
    apart from this process and is never killed. Done when it exits, or when another gateway shows
    in the state file."""
    before = read_json(state_file)
    try:
        process = subprocess.Popen(
            command, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            creationflags=getattr(subprocess, "DETACHED_PROCESS", 0), start_new_session=True,
        )
    except OSError:
        return False
    deadline = time.monotonic() + RESTART_TIMEOUT
    while time.monotonic() < deadline:
        code = process.poll()
        if code is not None:
            return code == 0
        record = read_json(state_file)
        if gateway_alive(record) and not same_gateway(record, before):
            return True
        time.sleep(1)
    return False


def restart_gateway(hermes_home: Path) -> bool:
    """``hermes gateway restart``: loads the plugin now, and starts a stopped gateway. Without
    ``--yes`` a newer Hermes refuses while it has live work (no prompt without a terminal), so a
    running turn or cron job is never cut off; the caller then asks for a manual restart."""
    hermes = find_hermes(hermes_home)
    if not hermes:
        return False
    root = hermes_root(hermes_home)
    env = hermes_env(root)
    profile = restart_profile(hermes_home)
    command = [hermes, "-p", profile, "gateway", "restart"]
    if not supports_detach(hermes, env):
        state_file = (root if profile == "default" else hermes_home) / "gateway_state.json"
        record = read_json(state_file)
        if gateway_alive(record) and record.get("active_agents"):
            return False  # no guard of its own: it would cut the running work off
        return await_new_gateway(command, env, state_file)
    command.append("--detach")
    try:
        result = subprocess.run(
            command, env=env,
            stdin=subprocess.DEVNULL, capture_output=True, text=True, encoding="utf-8",
            errors="replace", timeout=RESTART_TIMEOUT, check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return False
    return result.returncode == 0


def read_json(path: Path) -> dict:
    """A JSON object from ``path``; {} when missing, unreadable or anything else."""
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return value if isinstance(value, dict) else {}


def connector_homes(root: Path) -> List[Path]:
    """The default profile and named profiles that have the plugin. Refresh never adds it to one."""
    profiles = sorted(path for path in (root / "profiles").glob("*") if path.is_dir())
    return [home for home in [root, *profiles] if (home / "plugins" / INSTALL_DIR).is_dir()]


def is_current(home: Path) -> bool:
    target = home / "plugins" / INSTALL_DIR
    try:
        return all(filecmp.cmp(HERE / name, target / name, shallow=False) for name in PLUGIN_FILES)
    except OSError:
        return False


def gateway_alive(record: dict) -> bool:
    """Hermes's state file names a gateway that has not stopped. ``degraded`` still serves."""
    return isinstance(record.get("pid"), int) and record.get("gateway_state") not in ("stopped", "startup_failed")


def same_gateway(record: dict, pending: dict) -> bool:
    """The process in ``record`` is the one remembered. Any restart, ours or by hand, changes it."""
    now, then = record.get("start_time"), pending.get("start_time")
    close = not isinstance(now, int) or not isinstance(then, int) or abs(now - then) <= START_TIME_DRIFT
    return record.get("pid") == pending.get("pid") and close


def can_restart(record: dict) -> bool:
    """Serving now, by a fresh heartbeat. A stale one may be asleep or long dead, and
    `gateway restart` would start it: wait for the next run."""
    state = record.get("gateway_state")
    if state != "running" and not (state == "degraded" and record.get("exit_reason") is None):
        return False
    try:
        beat = datetime.fromisoformat(str(record.get("updated_at")))
        return abs((datetime.now(timezone.utc) - beat).total_seconds()) <= HEARTBEAT_FRESH
    except (TypeError, ValueError):
        return False


def forget_pending(pending_file: Path) -> None:
    try:
        pending_file.unlink()
    except OSError:
        pass


def refresh(root: Path, restart: bool = True) -> dict:
    """Replace outdated plugin files under ``root``, then settle the restart they need: ``needed``
    (``restart`` False: the caller does it), ``done``, ``busy`` or ``none``.

    A gateway that stopped reads the new files when it next starts. A refused restart, or a stale
    heartbeat, keeps the pending record, so the next run tries again."""
    result: dict = {"updated": [], "current": [], "failed": [], "restart": "none"}
    pending_file = root / "kuzz" / PENDING_FILE
    record = read_json(root / "gateway_state.json")
    for home in connector_homes(root):
        if is_current(home):
            result["current"].append(str(home))
            continue
        try:
            copy_plugin(home)
            result["updated"].append(str(home))
            if gateway_alive(record):
                pending_file.parent.mkdir(parents=True, exist_ok=True)
                pending_file.write_text(
                    json.dumps({"pid": record["pid"], "start_time": record.get("start_time")}), encoding="utf-8",
                )
        except OSError as exc:
            print(f"Could not update the Kuzz plugin in {home}: {exc}", file=sys.stderr)
            result["failed"].append(str(home))
    pending = read_json(pending_file)
    if not pending:
        return result
    if not gateway_alive(record) or not same_gateway(record, pending):
        forget_pending(pending_file)
    elif can_restart(record):
        if not restart:
            result["restart"] = "needed"
        else:
            result["restart"] = "done" if restart_gateway(root) else "busy"
    return result


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(description="Install the Kuzz plugin into Hermes Agent.")
    parser.add_argument("--owner", default=None, help="owner pubkey (hex or npub); default: the owner `kuzz pair` reports")
    parser.add_argument("--agent", default=None, help="the paired agent's pubkey: this Hermes home acts as it (KUZZ_PAIRED_AGENT)")
    parser.add_argument("--default-agent", action="append", default=[], metavar="OWNER=AGENT",
                        help="each owner's agent before this pairing; pins the default profile")
    parser.add_argument("--hermes-home", type=Path, default=None)
    parser.add_argument("--cli-path", default=None, help="path to the kuzz CLI (default: kuzz on PATH, then the Kuzz app's copy)")
    parser.add_argument("--no-enable", action="store_true", help="copy files only; do not enable the plugin")
    parser.add_argument("--no-restart", action="store_true", help="do not restart the Hermes gateway")
    parser.add_argument("--refresh", action="store_true", help="only update the plugin files of homes that have them")
    args = parser.parse_args(argv)

    if args.refresh:
        result = refresh(hermes_root(args.hermes_home or default_hermes_home()), restart=not args.no_restart)
        print(json.dumps(result))
        return 1 if result["failed"] else 0

    owner = normalize_pubkey(args.owner) if args.owner else None
    if args.owner and not owner:
        parser.error("--owner must be a 64-character hex pubkey or an npub")
    agent = normalize_pubkey(args.agent) if args.agent else None
    if args.agent and not agent:
        parser.error("--agent must be a 64-character hex pubkey or an npub")
    default_agents = {}
    for pair in args.default_agent:
        key, _, value = pair.partition("=")
        if not normalize_pubkey(key) or not normalize_pubkey(value):
            parser.error("--default-agent must be OWNER=AGENT, each a 64-character hex pubkey or an npub")
        default_agents[normalize_pubkey(key)] = normalize_pubkey(value)
    cli_path = args.cli_path or find_installed_kuzz()
    if not cli_path:
        parser.error("the kuzz CLI was not found; install Kuzz or pass --cli-path")
    hermes_home = args.hermes_home or default_hermes_home()

    pin_default_profile(hermes_home, default_agents)
    root = hermes_root(hermes_home)
    old_root = root != hermes_home and has_old_connector(root)
    if root != hermes_home and ((root / "plugins" / INSTALL_DIR).is_dir() or old_root):
        # The gateway restart reloads the default profile too; its copy must read scoped settings.
        copy_plugin(root)
    old_here = has_old_connector(hermes_home)
    target = copy_plugin(hermes_home)
    # Before any pin: a pin the gateway won't act on would read as connected.
    if not args.no_enable and not (
        enable_plugin(hermes_home, old_here) and (not old_root or enable_plugin(root, True))
    ):
        print(
            f"Could not enable the plugin. Add {PLUGIN_NAME} to plugins.enabled in "
            f"{hermes_home / 'config.yaml'}, then connect again.",
            file=sys.stderr,
        )
        return 1
    # Only now: a failed enable leaves the working connector in place.
    for home, old in ((hermes_home, old_here), (root, old_root)):
        if old and not args.no_enable:
            retire_old_connector(home)
    env_file = hermes_home / ".env"
    # A different agent or owner than the one this home was set up for: its home DM is stale.
    # So is one in a named profile with no pin yet, copied with a cloned profile's .env.
    previous = {name: read_env_value(env_file, name) for name in (PAIRED_AGENT_ENV, "KUZZ_OWNER_PUBKEY")}
    new_identity = bool(
        (agent and previous[PAIRED_AGENT_ENV] and normalize_pubkey(previous[PAIRED_AGENT_ENV]) != agent)
        or (owner and previous["KUZZ_OWNER_PUBKEY"] and normalize_pubkey(previous["KUZZ_OWNER_PUBKEY"]) != owner)
        or (agent and not previous[PAIRED_AGENT_ENV] and root != hermes_home)
    )
    values = {"KUZZ_CLI_PATH": cli_path}
    if owner:
        values["KUZZ_OWNER_PUBKEY"] = owner
    if agent:
        # Pin the agent: another pairing on this computer (a second profile) must not take over.
        values[PAIRED_AGENT_ENV] = agent
    set_env_values(env_file, values)
    print(f"Installed the Kuzz plugin in {target}")

    if not owner:
        print("Set KUZZ_OWNER_PUBKEY in .env: Hermes does not start the platform without an owner.", file=sys.stderr)
    if args.no_enable:
        print(f"Next: add {PLUGIN_NAME} to plugins.enabled in config.yaml, then hermes gateway restart")
        return 0
    if owner and set_home_channel(hermes_home, cli_path, owner, agent, replace=new_identity):
        print("Set the owner's DM as the Kuzz home channel")
    if args.no_restart:
        print("Next: hermes gateway restart")
    elif restart_gateway(hermes_home):
        print("Restarted the Hermes gateway")
    else:
        print("Could not restart the Hermes gateway (it may be busy). Next: hermes gateway restart", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
