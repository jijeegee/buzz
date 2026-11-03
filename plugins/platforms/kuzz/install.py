"""Install the Kuzz build of Hermes' ``buzz`` platform plugin into a Hermes home.

    python install.py [--owner PUBKEY] [--hermes-home DIR] [--cli-path PATH] [--no-enable]

What it does (the same steps `kuzz pair --hermes` and the desktop Hermes card follow):
1. copies PLUGIN_FILES into <HERMES_HOME>/plugins/buzz/. The manifest name is ``buzz-platform``,
   the bundled plugin's name, so Hermes loads this copy in its place (a user plugin wins a
   name collision) and every existing ``buzz`` setting keeps working;
2. sets KUZZ_CLI_PATH (the given path, else the installed kuzz) and, when given,
   KUZZ_OWNER_PUBKEY in <HERMES_HOME>/.env. KUZZ_CLI_PATH outranks an older BUZZ_CLI_PATH;
3. adds ``buzz-platform`` to ``plugins.enabled`` (and drops it from ``plugins.disabled``) with
   `hermes config get` / `hermes config set`. (`hermes plugins enable buzz-platform` resolves
   the name to the bundled plugin's listing key, ``platforms/buzz``, which the loader does not
   read for a user plugin.)
4. with an owner and no home channel yet, sets BUZZ_HOME_CHANNEL to the owner's DM (opened by
   the paired agent), so cron results reach Kuzz and Hermes does not ask for /sethome;
5. restarts the Hermes gateway (``--no-restart`` skips it) so the plugin loads now. A stopped
   gateway is started; a busy one is left alone and a manual restart is asked for.

`kuzz pair --hermes` runs ``python install.py --owner <hex> --hermes-home <dir>`` from its
install root: the kuzz binary's folder, ``<prefix>/share/kuzz``, or ``<local data>/kuzz``,
each holding ``plugins/platforms/kuzz/``. The kuzz binary in that root is preferred.
To go back to the bundled plugin, delete
<HERMES_HOME>/plugins/buzz/. Stdlib only; no keys are read or written.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import List, Optional

HERE = Path(__file__).resolve().parent
PLUGIN_NAME = "buzz-platform"  # manifest name; equal to the bundled plugin's, so ours replaces it
INSTALL_DIR = "buzz"
PLUGIN_FILES = ("plugin.yaml", "__init__.py", "adapter.py", "kuzz_cli.py")
HOME_ENV = "BUZZ_HOME_CHANNEL"
# `kuzz pair --hermes` gives claim + install 180 s; a restart waits up to ~55 s for the old gateway.
RESTART_TIMEOUT = 100

sys.path.insert(0, str(HERE))
from kuzz_cli import child_env, find_kuzz, is_runnable, normalize_pubkey, which_program  # noqa: E402  (stdlib-only)


def default_hermes_home() -> Path:
    env = os.environ.get("HERMES_HOME", "").strip()
    if env:
        return Path(env)
    local = os.environ.get("LOCALAPPDATA", "").strip()
    if os.name == "nt" and local:
        return Path(local) / "hermes"
    return Path.home() / ".hermes"


def copy_plugin(hermes_home: Path) -> Path:
    """Replace <hermes_home>/plugins/buzz with a fresh copy of PLUGIN_FILES. An existing folder
    is moved to <hermes_home>/plugin-backups/buzz-<UTC time>/ (outside plugins/, so Hermes does
    not load it), never deleted."""
    plugins = hermes_home / "plugins"
    target = plugins / INSTALL_DIR
    staging = plugins / f".{INSTALL_DIR}.installing"
    plugins.mkdir(parents=True, exist_ok=True)
    shutil.rmtree(staging, ignore_errors=True)
    staging.mkdir()
    for name in PLUGIN_FILES:
        shutil.copy2(HERE / name, staging / name)
    if target.exists():
        backups = hermes_home / "plugin-backups"
        backups.mkdir(parents=True, exist_ok=True)
        stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
        backup = backups / f"{INSTALL_DIR}-{stamp}"
        n = 1
        while backup.exists():
            n += 1
            backup = backups / f"{INSTALL_DIR}-{stamp}-{n}"
        target.rename(backup)
        print(f"Moved the previous {target} to {backup}")
    staging.rename(target)
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


def find_hermes(hermes_home: Path) -> Optional[str]:
    """The ``hermes`` launcher; on Windows only ``hermes.exe`` (a .cmd/.bat is never run)."""
    found = which_program("hermes")
    if found and is_runnable(found):
        return found
    name = "hermes.exe" if os.name == "nt" else "hermes"
    for folder in (hermes_home / "bin", hermes_home / "hermes-agent" / "venv" / "Scripts",
                   hermes_home / "hermes-agent" / "venv" / "bin"):
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


def _hermes_config(hermes: str, env: dict, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [hermes, "config", *args], env=env, stdin=subprocess.DEVNULL, capture_output=True,
        text=True, encoding="utf-8", errors="replace", check=False,
    )


def _read_list(hermes: str, env: dict, key: str) -> Optional[List[str]]:
    """``hermes config get <key>`` as a list of strings; [] when unset; None when unreadable."""
    result = _hermes_config(hermes, env, "get", key)
    if result.returncode != 0:
        return [] if "not set" in (result.stdout + result.stderr) else None
    text = result.stdout.strip()
    if text in ("", "[]", "null", "None"):
        return []
    items = []
    for line in text.splitlines():
        if not line.startswith("- "):
            return None  # not a plain list: leave the config alone
        items.append(line[2:].strip().strip("'\""))
    return items


def enable_plugin(hermes_home: Path) -> bool:
    """Add PLUGIN_NAME to ``plugins.enabled`` and drop it from ``plugins.disabled``."""
    hermes = find_hermes(hermes_home)
    if not hermes:
        return False
    env = dict(os.environ, HERMES_HOME=str(hermes_home))
    enabled = _read_list(hermes, env, "plugins.enabled")
    disabled = _read_list(hermes, env, "plugins.disabled")
    if enabled is None or disabled is None:
        return False
    if PLUGIN_NAME not in enabled:
        result = _hermes_config(hermes, env, "set", "plugins.enabled", json.dumps(enabled + [PLUGIN_NAME]))
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


def set_home_channel(hermes_home: Path, cli_path: str, owner: str) -> Optional[str]:
    """Make the owner's DM the Buzz home channel (cron results and notices) unless one is set.

    The home is written the way ``/sethome`` mirrors it, BUZZ_HOME_CHANNEL in .env, which the
    plugin seeds into the gateway config at start. A home already in .env or config.yaml is kept.
    Returns the DM id when one was set."""
    env_file = hermes_home / ".env"
    if read_env_value(env_file, HOME_ENV):
        return None
    hermes = find_hermes(hermes_home)
    if hermes:
        env = dict(os.environ, HERMES_HOME=str(hermes_home))
        if _hermes_config(hermes, env, "get", "platforms.buzz.home_channel").returncode == 0:
            return None
    # The paired agent opens (or finds) its DM with the owner; a stored person login is skipped.
    try:
        result = subprocess.run(
            [cli_path, "dms", "open", "--pubkey", owner], env=child_env(home=hermes_home),
            stdin=subprocess.DEVNULL, capture_output=True, text=True, encoding="utf-8",
            errors="replace", timeout=60, check=False,
        )
        dm_id = json.loads(result.stdout).get("dm_id") if result.returncode == 0 else None
    except (OSError, subprocess.SubprocessError, ValueError, AttributeError):
        dm_id = None
    if not isinstance(dm_id, str) or not dm_id.strip():
        return None
    set_env_values(env_file, {HOME_ENV: dm_id.strip(), f"{HOME_ENV}_NAME": "Owner DM"})
    return dm_id.strip()


def restart_gateway(hermes_home: Path) -> bool:
    """``hermes gateway restart --detach``: loads the plugin now, and starts a stopped gateway.
    Without ``--yes`` it refuses while Hermes has live work (no prompt without a terminal), so a
    running turn or cron job is never cut off; the caller then asks for a manual restart."""
    hermes = find_hermes(hermes_home)
    if not hermes:
        return False
    try:
        result = subprocess.run(
            [hermes, "gateway", "restart", "--detach"], env=dict(os.environ, HERMES_HOME=str(hermes_home)),
            stdin=subprocess.DEVNULL, capture_output=True, text=True, encoding="utf-8",
            errors="replace", timeout=RESTART_TIMEOUT, check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return False
    return result.returncode == 0


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(description="Install the Kuzz buzz plugin into Hermes Agent.")
    parser.add_argument("--owner", default=None, help="owner pubkey (hex or npub); default: the owner `kuzz pair` reports")
    parser.add_argument("--hermes-home", type=Path, default=None)
    parser.add_argument("--cli-path", default=None, help="path to the kuzz CLI (default: kuzz on PATH, then the Kuzz app's copy)")
    parser.add_argument("--no-enable", action="store_true", help="copy files only; do not enable the plugin")
    parser.add_argument("--no-restart", action="store_true", help="do not restart the Hermes gateway")
    args = parser.parse_args(argv)

    owner = normalize_pubkey(args.owner) if args.owner else None
    if args.owner and not owner:
        parser.error("--owner must be a 64-character hex pubkey or an npub")
    cli_path = args.cli_path or find_installed_kuzz()
    if not cli_path:
        parser.error("the kuzz CLI was not found; install Kuzz or pass --cli-path")
    hermes_home = args.hermes_home or default_hermes_home()

    target = copy_plugin(hermes_home)
    values = {"KUZZ_CLI_PATH": cli_path}
    if owner:
        values["KUZZ_OWNER_PUBKEY"] = owner
    set_env_values(hermes_home / ".env", values)
    print(f"Installed the Kuzz buzz plugin in {target}")

    if args.no_enable:
        print(f"Next: add {PLUGIN_NAME} to plugins.enabled in config.yaml, then hermes gateway restart")
        return 0
    if not enable_plugin(hermes_home):
        print(
            f"Could not enable the plugin. Add {PLUGIN_NAME} to plugins.enabled in "
            f"{hermes_home / 'config.yaml'}, then restart the gateway.",
            file=sys.stderr,
        )
        return 1
    if owner and set_home_channel(hermes_home, cli_path, owner):
        print("Set the owner's DM as the Buzz home channel")
    if args.no_restart:
        print("Next: hermes gateway restart")
    elif restart_gateway(hermes_home):
        print("Restarted the Hermes gateway")
    else:
        print("Could not restart the Hermes gateway (it may be busy). Next: hermes gateway restart", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
