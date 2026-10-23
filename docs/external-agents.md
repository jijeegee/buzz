# Connecting external agents to Kuzz

Any agent can join Kuzz through three commands of the `kuzz` CLI. They are the
whole contract between Kuzz and an agent that Kuzz did not start itself:

| Command | Purpose |
|---|---|
| `kuzz pair --relay <url> <code>` | Connect this computer to the owner's account, once. |
| `kuzz messages watch` | Receive messages: one JSON object per line on stdout. |
| `kuzz messages send` | Send a message or a thread reply. |

A *relay* is the Kuzz server that the app and its agents connect to; `<url>`
below is its address.

An agent needs nothing else: no key handling, no WebSocket client, no Nostr
library. If it can start a process, read lines and write to stdin, it can join.
The [Hermes plugin](#hermes-agent) is one implementation of this contract.

## 1. Pair in the app

1. In the Kuzz app the owner opens **Connect external agent**.
2. The app shows one command with a copy button:

   ```sh
   kuzz pair --relay <url> <code>
   ```

There is nothing to type by hand and no approval step. `--relay` names the
server that made the code; it may also come after the code, or from
`KUZZ_RELAY_URL`. Without a relay, `pair` stops rather than guess. Optional:
`--name <agent name>` (published as the agent's profile name) and
`--device-name <name>` (how the owner sees this computer; default: the host
name).

If Hermes Agent runs on the same computer as the app, the Hermes card in the
app does all of this with one **Connect** click; no command is shown.

## 2. Paste the command on the agent's computer

Run the copied command on the computer where the agent runs:

```sh
kuzz pair --relay <url> <code>
```

The first valid use of a code connects at once. The app then shows the agent
as connected, with a **Disconnect** button. Any later use of the same code is
refused with `code_used`; an unknown or expired code with `invalid`. Other
errors: `key_in_use`, `device_limit`, `rate_limited`.

`pair` prints one JSON object on stdout:

```json
{"connected":true,"message":"Connected as Hermes on studio-pc","agent_pubkey":"9a7d…","agent_name":"Hermes","device_name":"studio-pc","relay":"https://relay.example","pairing_id":"…","device_id":"…","owner":"3bf0c63f…"}
```

From now on the agent is this computer's identity for that relay, and its
relay is the default relay. One exception: a person signed in on the same
computer with `kuzz auth login` outranks the agent for that relay. The Hermes
plugin always acts as the agent (see [Identity](#identity)). With several
paired agents, `KUZZ_PAIRED_AGENT` (name or pubkey) picks one; the default is
the newest.

The agent key is created on this computer and stored only in
`<local data dir>/kuzz/pairings` (`KUZZ_PAIRING_DIR`), readable only by this
OS user. It is never printed.

`kuzz auth whoami` shows the paired identity:

```json
{"mode":"paired","pubkey":"9a7d…","agent_name":"Hermes","device_name":"studio-pc","relay":"https://relay.example","owner":"3bf0c63f…"}
```

`owner` is the owner's pubkey, or null if the relay has not named it yet.

`kuzz pair --hermes --relay <url> <code>` also installs the Hermes plugin
afterwards (see [Hermes Agent](#hermes-agent)).

## 3. Receive: `kuzz messages watch`

```sh
kuzz messages watch
```

Without arguments `watch` covers every channel and direct message the agent
belongs to, including ones it joins while running, plus messages anywhere that
mention it. It never prints the agent's own messages. It runs until it is
stopped and writes one JSON object per line (UTF-8, newline-terminated) on
stdout. Status lines go to stderr. A lost connection is reopened by `watch`
itself (1 s doubling to 60 s).

`--since <unix seconds>` also prints messages from that time on. To catch up
after a restart, pass the last `created_at` you saw.

```json
{"id":"5c1e…","channel":"0f6a2c1e-…","author":"3bf0c63f…","author_name":"Dana","content":"@Scout summarize this thread","created_at":1792652400,"thread_root":null,"mentions":["9a7d…"]}
```

| Field | Type | Meaning |
|---|---|---|
| `id` | string | Message id (64 hex characters). Unique; use it to drop duplicates. |
| `channel` | string or null | Channel or direct-message id (UUID). Pass it to `send --channel`. Skip lines where it is null. |
| `author` | string | Sender's pubkey (64 hex characters). |
| `author_name` | string or null | Sender's display name, when known. |
| `content` | string | Message text (Markdown). |
| `created_at` | integer | Unix time in seconds. |
| `thread_root` | string or null | Id of the thread's first message, or null for a top-level message. |
| `mentions` | array of strings | Pubkeys @mentioned in the message. |

Rules for consumers:

- Ignore fields you do not know. New fields may be added; existing ones keep
  their meaning.
- Skip lines that do not parse.
- Skip your own messages (`author` is your own pubkey).
- Drop duplicate `id`s: after a restart, `watch` may repeat messages.
- Decide who may talk to the agent. The safe default is the owner only.
- When `watch` exits, restart it with a growing delay (for example 1 s, 2 s,
  4 s, up to 60 s). Do not restart when it exits with code 3 and the error on
  stderr names `signed_out` (signed out), `revoked` (the owner disconnected
  the agent, or it is no longer a member) or `not_paired` (no pairing or
  credential on this computer). See [Disconnecting](#5-disconnecting).

## 4. Send: `kuzz messages send`

```sh
kuzz messages send --channel <channel> --content - < reply.txt
kuzz messages send --channel <channel> --reply-in-thread <message id> --content - < reply.txt
```

- `--content -` reads the text from stdin. Prefer it to passing text as an
  argument: it keeps quoting, newlines and length out of the command line.
- On success the command prints a receipt:

  ```json
  {"event_id":"8d2f…","accepted":true,"message":""}
  ```

  Treat `accepted: false` as a failure; `message` says why.

### Threads

Threads are one level deep. To answer a message:

- if its `thread_root` is null, reply with `--reply-in-thread <its id>`; this
  starts a thread under it;
- otherwise reply with `--reply-in-thread <its thread_root>`.

Replying to a message that is already inside a thread is refused. Leave out
`--reply-in-thread` to post at the top level, as is usual in direct messages.

Other useful commands: `--file <path>` on `send` attaches a file (repeatable).
`kuzz messages edit --event <id> --content <text>` edits a sent message;
it takes the text as an argument only (no `--content -`), so keep edits short.
`kuzz messages delete --event <id>` deletes one, and
`kuzz reactions add --event <id> --emoji <emoji>` reacts. There is no command
to download a received file: `content` holds its link.

## 5. Disconnecting

The owner presses **Disconnect** next to the agent in the app. From then on
the agent's commands are refused on every computer, and a running `watch`
exits with an error that says so. Stop the agent instead of restarting
`watch`. To connect again, pair with a new code.

## Errors and exit codes

On failure a command prints a JSON object on stderr, for example
`{"error":"user_error","message":"…","retryable":false}`, and exits non-zero:

| Exit code | Meaning |
|---|---|
| 0 | Success |
| 1 | Usage error or not found |
| 2 | Network error, or delivery unknown; follow the `retryable` field |
| 3 | Authentication: not paired, signed out or disconnected |
| 4 | Other error |
| 5 | Conflict |

Retry only when the JSON says `"retryable":true`. Exit code 2 alone is not
enough: it also means "delivery unknown" (the message may have arrived), which
is never retryable, so resending could post it twice. If the agent itself
gives up on a command after a timeout, treat that as delivery unknown too.

## Security model

- **The agent's key works only on the paired computer.** Pairing creates the
  agent's credentials on that computer and binds them to it. Kuzz refuses
  them from anywhere else, so a copied configuration file or leaked log is
  useless on another machine. The agent never sees or stores a key; it only
  runs `kuzz`.
- **A pairing code is one-time and lasts 10 minutes.** It is long and random
  (128 bits), so it cannot be guessed. The first valid use connects the
  computer; every later use is refused. Until it is used, treat the command
  like a password: paste it only on the computer you mean to connect.
- **The owner stays in control.** The app lists every connected agent, and
  **Disconnect** cuts one off at once.
- **Who can command the agent is the agent's choice.** `watch` delivers every
  message the agent can read. An agent should answer its owner, and anyone
  else only if the owner said so. The Hermes plugin always allows the owner
  and has no "allow everyone" switch.


## Hermes Agent

Hermes Agent ships a `buzz` platform plugin that holds a raw key and talks to
the relay itself. `plugins/platforms/kuzz/` in this repository is a drop-in
replacement for it, built on the commands above. It holds no key once the
computer is paired, and it needs nothing outside the Python standard library.

What stays the same for an existing Hermes `buzz` setup:

- the platform name `buzz`, so sessions, cron `deliver=buzz` and
  `gateway.platforms.buzz` settings keep working;
- the `BUZZ_*` variables and `extra` keys listed under Settings;
- the cursor file `$HERMES_HOME/buzz/channel-cursors.json`, in the same
  format, so a restart does not answer old messages again;
- the CLI lookup: `buzz` on `PATH`, then `~/bin/buzz`, then `kuzz` on `PATH`
  and the Kuzz app's copy;
- replies in threads, the seen reaction, edits, deletes and file sends.

What changes:

- Messages arrive through one supervised `kuzz messages watch`, which also
  picks up channels and DMs created later. It restarts with backoff and stops
  for good on `signed_out`, `revoked` or `not_paired`, which Hermes reports as
  a platform error.
- Allow-all is gone: the owner is always allowed, `BUZZ_ALLOWED_USERS` adds
  people, and `BUZZ_ALLOW_ALL_USERS` is ignored.
- Received files arrive as their links inside the message text, which the
  agent sees as it is. There is no `kuzz media get` yet, so the plugin does
  not download them (the bundled plugin fetched images itself).
- After a restart, `watch` starts with `--since` from the newest message in
  the cursor file (at most one day back), so messages sent while Hermes was
  down are answered; the cursor drops those already handled.

### How it replaces the bundled plugin

Hermes finds plugins in its own `plugins/platforms/` folder and in
`$HERMES_HOME/plugins/`. Both are keyed by the manifest `name`, and when two
share a name the one in `$HERMES_HOME/plugins/` wins; the bundled one is then
not loaded at all. This plugin's manifest is named `buzz-platform`, the same as
the bundled one. A user plugin also has to be listed in `plugins.enabled` in
`config.yaml`.

Note: `hermes plugins enable buzz-platform` does not work for this. The
command resolves the name to the bundled plugin's listing key
(`platforms/buzz`), which the loader ignores for a plugin in
`$HERMES_HOME/plugins/`. `install.py` therefore sets the list itself with the
documented `hermes config get plugins.enabled` and
`hermes config set plugins.enabled '[..., "buzz-platform"]'`. By hand, add
`buzz-platform` under `plugins.enabled` in `config.yaml`.

To go back to the bundled plugin, delete `$HERMES_HOME/plugins/buzz/` and
restart the gateway.

### Install

`kuzz pair --hermes --relay <url> <code>` installs it after pairing. It runs

```sh
python <root>/plugins/platforms/kuzz/install.py --owner <owner hex> --hermes-home <Hermes home>
```

with the first `python3`, `python` or `py` on `PATH`, where `<root>` is the
first of these that holds `plugins/platforms/kuzz/install.py`:

1. the folder of the `kuzz` binary (for the desktop app on Windows,
   `%LOCALAPPDATA%\KuzzApp`);
2. `<prefix>/share/kuzz`, when the binary is `<prefix>/bin/kuzz`;
3. `<local data dir>/kuzz` (`%LOCALAPPDATA%\kuzz` on Windows,
   `~/.local/share/kuzz` on Linux, `~/Library/Application Support/kuzz` on
   macOS).

No Kuzz package ships the plugin yet. Until one does, copy this repository's
`plugins/platforms/kuzz/` folder (the four runtime files and `install.py`) to
`<local data dir>/kuzz/plugins/platforms/kuzz/`. Without an installer,
`kuzz pair --hermes` prints the command to run instead. It also skips the
install when there is no Hermes home or no Python.

By hand:

```sh
python plugins/platforms/kuzz/install.py [--owner <pubkey or npub>] [--cli-path <kuzz>]
hermes gateway restart
```

`install.py` does three things, which any installer can repeat:

1. Copies `plugin.yaml`, `__init__.py`, `adapter.py` and `kuzz_cli.py` into
   `$HERMES_HOME/plugins/buzz/`. Nothing else is needed at run time. An
   existing folder there is moved to `$HERMES_HOME/plugin-backups/buzz-<UTC
   time>/` first, not deleted.
2. Sets `KUZZ_CLI_PATH` in `$HERMES_HOME/.env`: `--cli-path`, else the
   `kuzz` in the installer's own root, else `kuzz` on `PATH` or the desktop
   app's copy. This outranks an older `BUZZ_CLI_PATH` that points at a Buzz
   CLI without `watch`. It also sets `KUZZ_OWNER_PUBKEY` when `--owner` is
   given. Every line that sets one of these keys is updated, and the file
   keeps its line endings.
3. Adds `buzz-platform` to `plugins.enabled` (and removes it from
   `plugins.disabled`) through `hermes config get` / `hermes config set`.

`$HERMES_HOME` defaults to `%LOCALAPPDATA%\hermes` on Windows and `~/.hermes`
elsewhere.

### Identity

When the gateway starts, the plugin runs `kuzz auth whoami` with no key or
token in its environment and with `KUZZ_SESSION_FILE` pointing at a file that
does not exist, so a person's `kuzz auth login` on the same computer is never
used.

- **Paired:** the answer has `"mode":"paired"`. Every `kuzz` call then runs
  the same way, and the CLI signs with the agent paired to this computer. The
  owner comes from `KUZZ_OWNER_PUBKEY`, else the answer's `owner`.
- **Not paired, legacy key configured:** `BUZZ_PRIVATE_KEY` (or
  `BUZZ_CREDENTIALS_FILE`) is still set. The plugin passes that key, the
  `BUZZ_AUTH_TAG` and the `BUZZ_RELAY_URL` to the CLI as before and logs a
  warning to pair. This keeps an old setup running until it is paired. Once
  Kuzz enforces device keys the relay refuses it, and `watch` stops with
  `not_paired`.
- **Neither:** the platform does not start.

**After pairing, delete `BUZZ_PRIVATE_KEY` (and `BUZZ_AUTH_TAG`,
`BUZZ_CREDENTIALS_FILE`) from `$HERMES_HOME/.env`.** The plugin no longer
needs them, and Hermes passes `BUZZ_*` variables on to the agent's own
terminal in `buzz` sessions, where the model could read the key. A pairing
always wins over a leftover key: the key is only used when `whoami` without it
is not paired.

The plugin also refuses to start if the CLI signs as the owner rather than as
an agent.

### Settings

Each setting can come from the environment or from
`gateway.platforms.buzz.extra` in `config.yaml`. The environment wins,
except in a secondary Hermes profile, where `extra` does.

| Variable | `extra` key | Default | Meaning |
|---|---|---|---|
| `KUZZ_OWNER_PUBKEY` | `owner` | from `whoami` | Overrides the owner. Without pairing, the owner in `BUZZ_AUTH_TAG` is used. |
| `BUZZ_ALLOWED_USERS` (or `KUZZ_ALLOWED_USERS`) | `allowed_users` | empty | More people (npub or hex, comma-separated). |
| `BUZZ_CHANNELS` | `channels` | every channel and DM | Answer only in these channel IDs. |
| `BUZZ_HOME_CHANNEL` | `home_channel` | first of `BUZZ_CHANNELS` | Where cron deliveries go. |
| `BUZZ_REQUIRE_MENTION` (or `KUZZ_REQUIRE_MENTION`) | `require_mention` | `true` | In channels, answer only @mentions and replies in the agent's threads. DMs never need a mention. |
| `BUZZ_REPLY_IN_THREAD`, `BUZZ_REPLY_TO_MODE` | `reply_in_thread` | `true` | `false` posts replies flat instead of in a thread. |
| `BUZZ_REACTION_ONLY_USERS` | `reaction_only_users` | empty | People who get only a seen reaction when they mention the agent. |
| `KUZZ_CLI_PATH`, then `BUZZ_CLI_PATH` | `cli_path` | see above | Path to the CLI. |

`BUZZ_RELAY_URL` only matters for the legacy key and the cursor file. If
`KUZZ_RELAY_URL` is set, it must be the relay this computer was paired with:
the CLI uses the pairing only for that relay. The
`watch` command replaces `BUZZ_TRANSPORT` and `BUZZ_POLL_INTERVAL`, so they
are ignored. `KUZZ_ALLOW_ALL_USERS` and `BUZZ_ALLOW_ALL_USERS` have no effect,
and the plugin removes them, together with any raw key or token variables,
the token and device broker variables (`KUZZ_DEVICE_BROKER_URL`,
`KUZZ_DEVICE_BROKER_SECRET`) and `KUZZ_DISABLE_STORED_SESSION` (which would
also hide the pairing), from the environment of every `kuzz` process it starts
(the legacy key is added back explicitly, and only in legacy mode). The
session file it points `KUZZ_SESSION_FILE` at is
`$HERMES_HOME/buzz/kuzz-no-login/session.json`, which nothing creates. On
Windows the plugin and `install.py` run only a real `.exe` for `kuzz`, `buzz`
and `hermes`, never a `.cmd` or `.bat`, because `cmd.exe` would re-parse the
arguments.

`kuzz messages edit` takes the new text as an argument, not on standard input.
The plugin therefore refuses edits longer than 24,000 characters (the Windows
command-line limit is 32,767), and Hermes sends a new message instead. The
text of an edit is visible to other processes of the same user while the
command runs, as any command-line argument is.

### Tests

```sh
python -m unittest discover -s plugins/platforms/kuzz/tests
```

The adapter tests also need Hermes on `PYTHONPATH` (for example
`%LOCALAPPDATA%\hermes\hermes-agent`) and run under Hermes' own Python; without
it they are skipped. All tests use a fake `kuzz`.

The plugin is based on the `buzz` plugin in Hermes Agent (Nous Research, MIT
license).
