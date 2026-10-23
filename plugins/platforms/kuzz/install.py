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

`kuzz pair --hermes` runs ``python install.py --owner <hex> --hermes-home <dir>`` from its
install root: the kuzz binary's folder, ``<prefix>/share/kuzz``, or ``<local data>/kuzz``,
each holding ``plugins/platforms/kuzz/``. The kuzz binary in that root is preferred.
Restart the Hermes gateway afterwards. To go back to the bundled plugin, delete
<HERMES_HOME>/plugins/buzz/. Stdlib only; no keys are read or written.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import List, Optional

HERE = Path(__file__).resolve().parent
PLUGIN_NAME = "buzz-platform"  # manifest name; equal to the bundled plugin's, so ours replaces it
INSTALL_DIR = "buzz"
PLUGIN_FILES = ("plugin.yaml", "__init__.py", "adapter.py", "kuzz_cli.py")

sys.path.insert(0, str(HERE))
from kuzz_cli import find_kuzz, normalize_pubkey  # noqa: E402  (stdlib-only module)


def default_hermes_home() -> Path:
    env = os.environ.get("HERMES_HOME", "").strip()
    if env:
        return Path(env)
    local = os.environ.get("LOCALAPPDATA", "").strip()
    if os.name == "nt" and local:
        return Path(local) / "hermes"
    return Path.home() / ".hermes"


def copy_plugin(hermes_home: Path) -> Path:
    """Replace <hermes_home>/plugins/buzz with a fresh copy of PLUGIN_FILES."""
    plugins = hermes_home / "plugins"
    target = plugins / INSTALL_DIR
    staging = plugins / f".{INSTALL_DIR}.installing"
    plugins.mkdir(parents=True, exist_ok=True)
    shutil.rmtree(staging, ignore_errors=True)
    staging.mkdir()
    for name in PLUGIN_FILES:
        shutil.copy2(HERE / name, staging / name)
    shutil.rmtree(target, ignore_errors=True)
    staging.rename(target)
    return target


def set_env_values(env_file: Path, values: dict) -> None:
    """Upsert KEY=value lines in a dotenv file, leaving every other line untouched."""
    lines: List[str] = []
    if env_file.exists():
        lines = env_file.read_text(encoding="utf-8").splitlines()
    pending = dict(values)
    for i, line in enumerate(lines):
        key = line.split("=", 1)[0].strip()
        if key.startswith("export "):
            key = key[len("export "):].strip()
        if key in pending:
            lines[i] = f"{key}={pending.pop(key)}"
    lines += [f"{key}={value}" for key, value in pending.items()]
    env_file.parent.mkdir(parents=True, exist_ok=True)
    with open(env_file, "w", encoding="utf-8", newline="\n") as fh:
        fh.write("\n".join(lines) + "\n")


def find_hermes(hermes_home: Path) -> Optional[str]:
    found = shutil.which("hermes")
    if found:
        return found
    for folder in (hermes_home / "bin", hermes_home / "hermes-agent" / "venv" / "Scripts",
                   hermes_home / "hermes-agent" / "venv" / "bin"):
        for name in ("hermes.exe", "hermes.cmd", "hermes"):
            candidate = folder / name
            if candidate.is_file():
                return str(candidate)
    return None


def find_installed_kuzz() -> str:
    """The kuzz binary of the install root this file ships in, else :func:`find_kuzz`."""
    root = HERE.parents[2]  # <root>/plugins/platforms/kuzz/install.py
    candidates = [root / "kuzz.exe", root / "kuzz"]
    if root.name == "kuzz" and root.parent.name == "share":  # <prefix>/share/kuzz -> <prefix>/bin/kuzz
        candidates.append(root.parent.parent / "bin" / "kuzz")
    for candidate in candidates:
        if candidate.is_file():
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


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(description="Install the Kuzz buzz plugin into Hermes Agent.")
    parser.add_argument("--owner", default=None, help="owner pubkey (hex or npub); default: the owner `kuzz pair` reports")
    parser.add_argument("--hermes-home", type=Path, default=None)
    parser.add_argument("--cli-path", default=None, help="path to the kuzz CLI (default: kuzz on PATH, then the Kuzz app's copy)")
    parser.add_argument("--no-enable", action="store_true", help="copy files only; do not enable the plugin")
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
    print("Next: hermes gateway restart")
    return 0


if __name__ == "__main__":
    sys.exit(main())
