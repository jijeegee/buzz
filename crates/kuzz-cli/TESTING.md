# kuzz-cli Live Testing Guide

Manual testing runbook for verifying every CLI command against a local relay.
An agent or developer follows this step by step, running each command and
checking the output.

---

## 1. Prerequisites

Docker services running and healthy:

```bash
docker compose ps
# kuzz-postgres   healthy
# kuzz-redis      healthy
```

If not running: `just setup` from the repo root.

Tools: `jq`, `curl`, Rust toolchain.

---

## 2. Build the CLI

```bash
cargo build -p kuzz-cli
```

Use `cargo run -p kuzz-cli --` or the built binary at `target/debug/kuzz`.

---

## 3. Start the Relay

In a separate terminal:

```bash
cd REPOS/kuzz-nostr
set -a && source .env && set +a
cargo run -p kuzz-relay
```

Verify:

```bash
curl -s http://localhost:3000/_liveness
# "ok" or 200 status
```

The `.env` should have `KUZZ_REQUIRE_AUTH_TOKEN=false` for local dev.

---

## 4. Mint Test Credentials

### Option A: kuzz-admin (full scopes including admin)

This mints a token with all CLI-relevant scopes (including `admin:channels`)
via direct DB access. Use this for testing admin operations (archive,
delete-channel, add/remove-channel-member).

```bash
DATABASE_URL="${DATABASE_URL:?set DATABASE_URL for the local Kuzz database}" \
cargo run -p kuzz-admin -- mint-token \
  --name "cli-test" \
  --scopes "messages:read,messages:write,channels:read,channels:write,users:read,users:write,files:read,files:write,admin:channels"
```

This generates a keypair and prints:
- **Private key (nsec)** — save for `KUZZ_PRIVATE_KEY` testing

Export:

```bash
export KUZZ_RELAY_URL="http://localhost:3000"
export KUZZ_PRIVATE_KEY="nsec1..."   # from the mint output
```

### Scope reference

| Scope | Self-mintable | Needed for |
|-------|:---:|------------|
| `messages:read` | ✅ | `messages get`, `messages thread`, `messages search`, `feed get` |
| `messages:write` | ✅ | `messages send`, `messages edit`, `messages delete`, `reactions`, `messages vote` |
| `channels:read` | ✅ | `channels list`, `channels get`, `channels members` |
| `channels:write` | ✅ | `channels create`, `channels update`, `channels join`, `channels leave`, `channels topic`, `channels purpose` |
| `users:read` | ✅ | `users get`, `users presence` |
| `users:write` | ✅ | `users set-profile`, `users set-presence`, `users set-status` |
| `files:read` | ✅ | — |
| `files:write` | ✅ | — |
| `admin:channels` | ❌ | `channels archive`, `channels unarchive`, `channels delete`, `channels add-member`, `channels remove-member` |

---

## 5. Unit Tests

```bash
cargo test -p kuzz-cli
# Expected: see cargo test -p kuzz-cli for current count

cargo clippy -p kuzz-cli -- -D warnings
# Expected: zero warnings
```

---

## 5a. Token Mode (centralized identity, Phase 1)

With a relay running `AUTH_TOKEN_ENABLED=true`, the CLI authenticates with a
bearer token instead of `KUZZ_PRIVATE_KEY`. Sources, highest first:

1. `KUZZ_BOT_TOKEN` (`bzb_…`, injected into Desktop-hosted agents' `kuzz-acp`)
2. `KUZZ_ACCESS_TOKEN` (`bzs_…` user session or `bzk_…` headless bot token)
3. `KUZZ_TOKEN_BROKER_URL` + `KUZZ_TOKEN_BROKER_SECRET` (the `kuzz-acp`
   loopback broker; this is what `kuzz` sees inside a hosted agent)
4. otherwise key mode: `KUZZ_PRIVATE_KEY` (+ optional `KUZZ_AUTH_TAG`)
5. otherwise the login stored by `kuzz auth login` for this relay (§5b)

Tokens are env-only; there is no argv flag that would expose one in the
process list. An explicit `--private-key` on the command line always selects
key mode, even when a token env var is set. A rate-limited or unreachable
broker is a retryable relay error (exit 2); a rejected token or broker secret
is an auth error (exit 3).

On startup the CLI calls `GET /auth/me` with the token to learn its principal
id; events are then sent as drafts (`pubkey` = principal, all-zero `sig`) that
the relay stamps, and every bridge call carries `Authorization: Bearer`.

```bash
unset KUZZ_PRIVATE_KEY
export KUZZ_ACCESS_TOKEN=bzs_...   # from /auth/oidc/complete (dev: AUTH_OIDC_FAKE=1)
./target/release/kuzz --format compact channels list
# Expected: same output as key mode, authored by the principal id

KUZZ_ACCESS_TOKEN=bzs_revoked ./target/release/kuzz channels list; echo "exit=$?"
# Expected: {"error":"auth_error","message":"auth error: access token rejected: ..."} exit=3
```

`kuzz upload` / `kuzz media` send `Authorization: Bearer` in token mode (the
relay binds the body to `X-SHA-256`); expect the same descriptor as key mode.
Commands that need a secret key (`kuzz mem`, `kuzz gifs`, owner-reviewed
`agents draft-*` and project channel drafts, which NIP-44 encrypt to the
owner) fail with exit 1 and a message naming the reason.
Unit coverage lives in `src/client_token_tests.rs`.

---

## 5b. `kuzz auth` — human login and bot management (Phase 2)

`kuzz auth login` signs a human in with Google through the relay (OIDC +
PKCE, plan §3.2 CLI row): the CLI listens on `http://127.0.0.1:<random>/cb`,
opens the browser at `/auth/oidc/google/start?client=cli&…`, checks the
returned `state`, and posts the code with its PKCE verifier to
`/auth/oidc/complete`. It waits 5 minutes at most.

Storage, per relay origin (`scheme://host:port`):

- access token (`bzs_…`, 1 h) in `session.json` under the user config dir
  (`~/.config/kuzz/`, `%APPDATA%\kuzz\`, `~/Library/Application Support/kuzz/`)
  or `KUZZ_SESSION_FILE`; written atomically, `0600` on unix;
- refresh token (`bzr_…`, 90 days, rotating) in the OS credential store
  (service `kuzz-cli`, account `refresh:<origin>`). Without one (headless
  Linux) it falls back to `session.json` with a warning and
  `"refresh_storage": "file"` in the login result.

The stored login is the **last** credential source (§5a): any token env var
or a private key wins, so a human login never replaces an identity an agent
was given. Other commands use it automatically and refresh it 2 minutes
before expiry, persisting the rotated refresh token before anything else. A
refresh the relay rejects (`invalid_token`, `token_expired`,
`token_revoked`, `refresh_reused`, `principal_disabled`) forgets the login and
exits 3 with "run `kuzz auth login`"; a network/5xx failure keeps it (exit 2).

```bash
export KUZZ_RELAY_URL=http://127.0.0.1:3000      # relay with AUTH_TOKEN_ENABLED=true
unset KUZZ_PRIVATE_KEY KUZZ_ACCESS_TOKEN KUZZ_BOT_TOKEN
kuzz auth login                 # browser opens; dev relay: AUTH_OIDC_FAKE=1 signs in at once
# stdout: {"accepted":true,"message":"signed in","relay":"http://127.0.0.1:3000",
#          "principal_id":"<hex>","device_id":"<uuid>","refresh_storage":"keyring"}
kuzz auth whoami                # {"mode":"token","source":"session","principal_id":...}
kuzz --format compact channels list            # acts as the signed-in user
kuzz auth profile --display-name "Ada"
kuzz --format compact auth devices list        # [{"id","name","platform":"cli","current":true}]
kuzz auth sessions revoke-others               # other human devices only; bots untouched
kuzz auth bots create --name "home bot"        # {"bot_id":"<hex>"} (headless by default)
kuzz auth bots headless-token <bot_id>         # {"token":"bzk_…","hash_prefix":"…"} shown once
kuzz auth bots revoke-headless <bot_id> <hash_prefix>   # second step of a rotation
kuzz auth bots profile <bot_id> --display-name "Home"
kuzz auth bots revoke-all                      # every bot token you own
kuzz auth logout                # revokes this device + bots it hosts, then forgets it
kuzz channels list; echo "exit=$?"             # exit=3, "run `kuzz auth login`"
```

`kuzz auth logout` keeps the stored login when the relay cannot be reached
(exit 2) so the revoke can be retried; `--local-only` forgets it anyway. Bot
commands with `--host this-device` create a bot whose short-lived token
(`kuzz auth bots token <id>`) can only be issued from this CLI device.
Unit coverage: `src/commands/auth_tests.rs` (fake relay; in-memory credential
store — tests never touch the OS keyring) and `src/auth_loopback.rs`.

---

## 6. Live Testing — Command by Command

Run each command, verify exit code 0 and check output. Most commands
return JSON (pipe through `jq .` to validate). Commands are ordered so
earlier ones create resources that later ones need.

### 6.1 Channels

```bash
# channels create (stream)
kuzz channels create --name "test-stream" --type stream --visibility open \
  --description "CLI test channel" | jq .
# Save the channel ID:
CHANNEL_ID=$(kuzz channels create --name "test-cli" --type stream --visibility open | jq -r '.channel_id')
# Expected: {"event_id":"...","accepted":true,"message":"...","channel_id":"<uuid>"}

# channels create (forum) — needed for messages vote later
FORUM_ID=$(kuzz channels create --name "test-forum" --type forum --visibility open | jq -r '.channel_id')

# channels list
kuzz channels list | jq .
# Expected: [{"channel_id":"...","name":"...","description":"...","created_at":N}]
kuzz channels list --visibility open | jq .
kuzz channels list --member | jq .

# channels get
kuzz channels get --channel "$CHANNEL_ID" | jq .
# Expected: {"channel_id":"...","name":"...","description":"...","created_at":N,"pubkey":"..."} or null

# channels update
kuzz channels update --channel "$CHANNEL_ID" --name "test-cli-updated" \
  --description "Updated" | jq .
# Expected: {"event_id":"...","accepted":true,"message":"..."}

# channels topic
kuzz channels topic --channel "$CHANNEL_ID" --topic "Test topic" | jq .
# Expected: {"event_id":"...","accepted":true,"message":"..."}

# channels purpose
kuzz channels purpose --channel "$CHANNEL_ID" --purpose "Testing" | jq .
# Expected: {"event_id":"...","accepted":true,"message":"..."}

# channels join (may already be a member from create)
kuzz channels join --channel "$CHANNEL_ID" | jq .
# Expected: {"event_id":"...","accepted":true,"message":"..."}

# channels leave
# NOTE: Fails with 400 "cannot remove the last owner" if this identity is the
# sole owner (which it is after channels create). To test leave successfully,
# first add-member a second pubkey as owner. The relay enforces ≥1 owner.
kuzz channels leave --channel "$CHANNEL_ID" | jq .
# Expected: {"event_id":"...","accepted":true,"message":"..."} (or 400 if last owner)

# Re-join so we can send messages
kuzz channels join --channel "$CHANNEL_ID" | jq .
# Expected: {"event_id":"...","accepted":true,"message":"..."}

# channels archive (requires admin:channels scope)
kuzz channels archive --channel "$CHANNEL_ID" | jq .
# Expected: {"event_id":"...","accepted":true,"message":"..."}

# channels unarchive
kuzz channels unarchive --channel "$CHANNEL_ID" | jq .
# Expected: {"event_id":"...","accepted":true,"message":"..."}
```

### 6.2 Canvas

```bash
# canvas set
kuzz canvas set --channel "$CHANNEL_ID" --content "# Test Canvas" | jq .

# canvas set from stdin
echo "# Canvas from stdin" | kuzz canvas set --channel "$CHANNEL_ID" --content - | jq .

# canvas get
kuzz canvas get --channel "$CHANNEL_ID"
# Expected: raw markdown string, or: null
```

### 6.3 Messages

```bash
# messages send
MSG=$(kuzz messages send --channel "$CHANNEL_ID" --content "Hello from CLI test" | jq .)
echo "$MSG"
EVENT_ID=$(echo "$MSG" | jq -r '.event_id')

# messages send with reply + broadcast
REPLY=$(kuzz messages send --channel "$CHANNEL_ID" --content "Reply" \
  --reply-in-thread "$EVENT_ID" --broadcast | jq .)
echo "$REPLY"
REPLY_ID=$(echo "$REPLY" | jq -r '.event_id')

# messages send with mentions — @name in content is auto-resolved, no flag needed
kuzz messages send --channel "$CHANNEL_ID" --content "Hey @someone" | jq .

# messages send with NIP-27 nostr:npub1… inline mention — auto-resolved to p-tag
kuzz messages send --channel "$CHANNEL_ID" \
  --content "Check with nostr:npub10elfcs4fr0l0r8af98jlmgdh9c8tcxjvz9qkw038js35mp4dma8qzvjptg on this" | jq .

# messages send from stdin — safe path for content with shell metacharacters
# (backticks, $vars, code blocks) that would otherwise be expanded by the shell.
echo 'Body with `backticks` and $vars stays literal.' \
  | kuzz messages send --channel "$CHANNEL_ID" --content - | jq .

# messages get
kuzz messages get --channel "$CHANNEL_ID" | jq .
kuzz messages get --channel "$CHANNEL_ID" --limit 5 | jq .

# messages thread from the root, a reply, and a canonical link
kuzz messages thread --channel "$CHANNEL_ID" --event "$EVENT_ID" | jq .
kuzz messages thread --channel "$CHANNEL_ID" --event "$REPLY_ID" | jq .
kuzz messages thread \
  --link "kuzz://message?channel=$CHANNEL_ID&id=$REPLY_ID&thread=$EVENT_ID" | jq .

# messages search
kuzz messages search --query "Hello" | jq .
kuzz messages search --query "CLI test" --limit 5 | jq .

# messages edit
kuzz messages edit --event "$EVENT_ID" --content "Edited by CLI test" | jq .

# messages delete
kuzz messages delete --event "$REPLY_ID" | jq .
```

### 6.4 Diff Messages

```bash
# messages send-diff from stdin
echo '--- a/foo.rs
+++ b/foo.rs
@@ -1,3 +1,3 @@
-fn old() {}
+fn new() {}' | kuzz messages send-diff \
  --channel "$CHANNEL_ID" \
  --diff - \
  --repo "https://github.com/example/repo" \
  --commit "abcdef1234567890abcdef1234567890abcdef12" | jq .

# messages send-diff with metadata
echo "diff content" | kuzz messages send-diff \
  --channel "$CHANNEL_ID" \
  --diff - \
  --repo "https://github.com/example/repo" \
  --commit "abcdef1234567890abcdef1234567890abcdef12" \
  --file "src/main.rs" \
  --lang "rust" \
  --description "Refactored main" | jq .

# messages send-diff with branch + PR metadata
echo "diff content" | kuzz messages send-diff \
  --channel "$CHANNEL_ID" \
  --diff - \
  --repo "https://github.com/example/repo" \
  --commit "abcdef1234567890abcdef1234567890abcdef12" \
  --parent-commit "1234567890abcdef1234567890abcdef12345678" \
  --source-branch "feature/cli" \
  --target-branch "main" \
  --pr 42 | jq .
```

### 6.5 Reactions

```bash
# Send a message to react to
REACT_MSG=$(kuzz messages send --channel "$CHANNEL_ID" --content "React to this")
REACT_ID=$(echo "$REACT_MSG" | jq -r '.event_id')

# reactions add
kuzz reactions add --event "$REACT_ID" --emoji "👍" | jq .

# reactions get
kuzz reactions get --event "$REACT_ID" | jq .
# Expected: {"reactions":[{"emoji":"...","count":N,"pubkeys":["..."]}]}

# reactions remove
kuzz reactions remove --event "$REACT_ID" --emoji "👍" | jq .
```

### 6.6 DMs

```bash
# dms list
kuzz dms list | jq .
# Expected: [{"dm_id":"...","participants":["..."],"created_at":N}]

# dms open (needs a real pubkey — use your own or a test one)
# Get your own pubkey first:
MY_PUBKEY=$(kuzz users get | jq -r '.[0].pubkey // empty')
echo "My pubkey: $MY_PUBKEY"

# dms open with a synthetic pubkey (relay will create the user)
DM_RESULT=$(kuzz dms open --pubkey "0000000000000000000000000000000000000000000000000000000000000001")
echo "$DM_RESULT" | jq .
# Expected: {"event_id":"...","accepted":true,"message":"...","dm_id":"<uuid>"}
DM_ID=$(echo "$DM_RESULT" | jq -r '.dm_id')

# dms add-member (requires messages:write scope — NOT admin:channels)
kuzz dms add-member --channel "$DM_ID" \
  --pubkey "0000000000000000000000000000000000000000000000000000000000000002" | jq .
```

### 6.7 Users & Presence

```bash
# users get — own profile (0 pubkeys)
kuzz users get | jq .
# Expected: [{...profile...}] — always returns an array, even for single results

# users get — single pubkey
kuzz users get --pubkey "$MY_PUBKEY" | jq .

# users get — batch (2+ pubkeys)
kuzz users get --pubkey "$MY_PUBKEY" --pubkey "$MY_PUBKEY" | jq .

# users set-profile
kuzz users set-profile --name "CLI Test Agent" --about "Testing kuzz-cli" | jq .

# users presence
kuzz users presence --pubkeys "$MY_PUBKEY" | jq .

# users set-presence
kuzz users set-presence --status online | jq .
kuzz users set-presence --status away | jq .
kuzz users set-presence --status offline | jq .
# Note: set-presence may fail — kind:20001 is ephemeral and rejected by the HTTP bridge

# users set-status — NIP-38 kind:30315 on the d:general coordinate
kuzz users set-status --text "reviewing PRs" --emoji "🔍" | jq .
kuzz users set-status --text "no emoji this time" | jq .

# users set-status — emoji-only status (intentional: text is blank, emoji is kept)
kuzz users set-status --text "" --emoji "🎶" | jq .

# users set-status --clear — removes the status (empty content, d:general only)
kuzz users set-status --clear | jq .

# --clear is mutually exclusive with --text/--emoji
kuzz users set-status --clear --text "nope" 2>&1; echo "exit: $?"
# Expected: exit 1 — clap conflict error
```

### 6.8 Channel Members (add/remove require admin:channels)

```bash
# channels add-member
kuzz channels add-member --channel "$CHANNEL_ID" \
  --pubkey "0000000000000000000000000000000000000000000000000000000000000001" \
  --role member | jq .

# channels members
kuzz channels members --channel "$CHANNEL_ID" | jq .
# Expected: [{"pubkey":"...","role":"..."}]

# channels remove-member
kuzz channels remove-member --channel "$CHANNEL_ID" \
  --pubkey "0000000000000000000000000000000000000000000000000000000000000001" | jq .
```

### 6.9 Workflows

```bash
# workflows create
# NOTE: trigger uses `on:` tag (serde internally tagged enum).
# Valid triggers: message_posted, reaction_added, diff_posted, schedule, webhook
# Steps use `action:` tag: send_message, send_dm, set_channel_topic, add_reaction, etc.
WF=$(kuzz workflows create --channel "$CHANNEL_ID" \
  --yaml 'name: test-wf
trigger:
  on: webhook
steps:
  - id: step1
    action: send_message
    text: "Hello from workflow"' | jq .)
echo "$WF"
WF_ID=$(echo "$WF" | jq -r '.workflow_id')

# workflows list
kuzz workflows list --channel "$CHANNEL_ID" | jq .

# workflows get
kuzz workflows get --workflow "$WF_ID" | jq .
# Expected: {"workflow_id":"...","content":"<yaml>","created_at":N,"pubkey":"..."} or null

# workflows update (requires --channel)
kuzz workflows update --channel "$CHANNEL_ID" --workflow "$WF_ID" \
  --yaml 'name: test-wf-updated
trigger:
  on: webhook
steps:
  - id: step1
    action: send_message
    text: "Updated"' | jq .

# workflows trigger
# NOTE: May return 400 "workflow not found" — the relay indexes workflow
# definitions into a DB table asynchronously. If the definition event hasn't
# been indexed yet, the trigger handler won't find it.
kuzz workflows trigger --workflow "$WF_ID" | jq .

# workflows runs
kuzz workflows runs --workflow "$WF_ID" | jq .
# Expected: [] — relay stores runs in DB, not as Nostr events; empty is normal

# workflows approve — requires a workflow run waiting for approval
# This is hard to test ad-hoc without a workflow that has an approval gate.
# Test the validation instead:
kuzz workflows approve --token "00000000-0000-0000-0000-000000000000" 2>&1 || true
# Should fail with relay error (token not found), not a validation error
# To test the deny path: kuzz workflows approve --token <UUID> --approved false

# workflows delete
kuzz workflows delete --workflow "$WF_ID" | jq .
```

### 6.10 Feed

```bash
kuzz feed get | jq .
kuzz feed get --limit 5 | jq .
# Expected: complete signed Nostr events with
# {id,pubkey,kind,content,created_at,sig,tags}, sorted newest-first
```

### 6.11 Forum & Voting

```bash
# Send a forum post (kind 45001) to the forum channel
FORUM_POST=$(kuzz messages send --channel "$FORUM_ID" \
  --content "Forum post for vote testing" --kind 45001 | jq .)
echo "$FORUM_POST"
FORUM_EVENT_ID=$(echo "$FORUM_POST" | jq -r '.event_id')

# messages vote (up)
kuzz messages vote --event "$FORUM_EVENT_ID" --direction up | jq .

# messages vote (down)
kuzz messages vote --event "$FORUM_EVENT_ID" --direction down | jq .
```

### 6.12 Notes (NIP-23 long-form, kind:30023)

Editable team-knowledge notes keyed by `(kind:30023, you, d=slug)`. `set` is an
idempotent upsert; `rm` is a NIP-09 a-tag deletion. Output is plain text (refs),
not JSON — except `get`/`ls`, which emit JSON.

```bash
# set (first publish — --title required, body from stdin)
cat <<'EOF' | kuzz notes set --name dco-check --title "DCO Check" \
  --summary "How we verify DCO" --tag dco --tag ci --content -
Run `git log --format='%(trailers:key=Signed-off-by)'` ...
EOF
# → prints event_id / naddr / coordinate / slug / title

# set (edit — omit --title to carry it forward; published_at preserved)
echo "Updated body." | kuzz notes set --name dco-check --content -

# get by name (own author resolves directly; cross-author #d query otherwise)
kuzz notes get --name dco-check | jq .
kuzz notes get --name dco-check --content-only

# get by naddr (exact coordinate; paste the naddr from a set/get above)
kuzz notes get --naddr "$NADDR" | jq .

# ls (own by default; --author all across the team; --tag filters)
kuzz notes ls | jq .
kuzz notes ls --tag dco | jq .
kuzz notes ls --author all --limit 10 | jq .

# rm (NIP-09 a-tag deletion; subsequent get must 404)
kuzz notes rm --name dco-check
# → prints deleted <coordinate> / deletion <event-id>
kuzz notes get --name dco-check   # exits non-zero: not found

# rm of a slug you never published → NotFound, no kind:5 emitted
kuzz notes rm --name does-not-exist   # exits non-zero
```

---

## 7. Error Path Testing

Verify the CLI produces correct JSON on stderr and correct exit codes.

```bash
# Exit 1: Invalid UUID
kuzz channels get --channel "not-a-uuid" 2>&1; echo "exit: $?"
# stderr: {"error":"user_error","message":"invalid UUID: not-a-uuid"}
# exit: 1

# Exit 1: Invalid hex64
kuzz messages delete --event "not-hex" 2>&1; echo "exit: $?"
# stderr: {"error":"user_error","message":"must be a 64-character hex string: not-hex"}
# exit: 1

# Exit 1: Invalid --type value (clap validates the enum — multi-line error)
kuzz channels create --name x --type invalid --visibility open 2>&1; echo "exit: $?"
# stderr: {"error":"user_error","message":"error: invalid value 'invalid' for '--type <CHANNEL_TYPE>'\n  [possible values: stream, forum]\n..."}
# exit: 1

# Exit 1: Invalid --direction value
kuzz messages vote --event "$(printf '0%.0s' {1..64})" \
  --direction sideways 2>&1; echo "exit: $?"
# exit: 1

# Exit 1: Empty body guard
kuzz users set-profile 2>&1; echo "exit: $?"
# exit: 1 (at least one field required)

# Exit 3: No auth configured
env -u KUZZ_PRIVATE_KEY KUZZ_SESSION_FILE=/nonexistent/session.json \
  cargo run -p kuzz-cli -- channels list 2>&1; echo "exit: $?"
# stderr: {"error":"auth_error","message":"auth error: no credential: run `kuzz auth login`, or set KUZZ_PRIVATE_KEY (--private-key) or KUZZ_BOT_TOKEN / KUZZ_ACCESS_TOKEN / KUZZ_TOKEN_BROKER_URL"}
# exit: 3

# Not-found returns null, not an error (exit 0)
kuzz channels get --channel "00000000-0000-0000-0000-000000000000"
# stdout: null
# exit: 0
```

---

## 8. Auth Testing

Test authentication.

```bash
# Private key (KUZZ_PRIVATE_KEY)
KUZZ_PRIVATE_KEY="nsec1..." kuzz channels list | jq .
# Should succeed

# No auth → exit 3
env -u KUZZ_PRIVATE_KEY KUZZ_SESSION_FILE=/nonexistent/session.json \
  cargo run -p kuzz-cli -- channels list 2>&1; echo "exit: $?"
# stderr: {"error":"auth_error","message":"auth error: no credential: run `kuzz auth login`, or set KUZZ_PRIVATE_KEY (--private-key) or KUZZ_BOT_TOKEN / KUZZ_ACCESS_TOKEN / KUZZ_TOKEN_BROKER_URL"}
# exit: 3
```

---

## 9. Cleanup

```bash
# Delete test channels
kuzz channels delete --channel "$CHANNEL_ID" | jq .
kuzz channels delete --channel "$FORUM_ID" | jq .
```

---

## 10. Checklist

| # | Command | Tested | Notes |
|---|---------|:------:|-------|
| 1 | `messages send` | ☐ | Basic, reply, broadcast, mentions, stdin |
| 2 | `messages send-diff` | ☐ | Stdin, metadata, branch/PR |
| 3 | `messages edit` | ☐ | |
| 4 | `messages delete` | ☐ | |
| 5 | `messages get` | ☐ | With limit |
| 6 | `messages thread` | ☐ | |
| 7 | `messages search` | ☐ | With limit |
| 8 | `messages vote` | ☐ | Up and down |
| 9 | `channels list` | ☐ | With visibility, member |
| 10 | `channels get` | ☐ | |
| 11 | `channels create` | ☐ | Stream and forum |
| 12 | `channels update` | ☐ | |
| 13 | `channels topic` | ☐ | |
| 14 | `channels purpose` | ☐ | |
| 15 | `channels join` | ☐ | |
| 16 | `channels leave` | ☐ | |
| 17 | `channels archive` | ☐ | Needs admin:channels |
| 18 | `channels unarchive` | ☐ | Needs admin:channels |
| 19 | `channels delete` | ☐ | Needs admin:channels |
| 20 | `channels members` | ☐ | |
| 21 | `channels add-member` | ☐ | Needs admin:channels |
| 22 | `channels remove-member` | ☐ | Needs admin:channels |
| 23 | `canvas get` | ☐ | |
| 24 | `canvas set` | ☐ | Direct and stdin |
| 25 | `reactions add` | ☐ | |
| 26 | `reactions remove` | ☐ | |
| 27 | `reactions get` | ☐ | |
| 28 | `dms list` | ☐ | |
| 29 | `dms open` | ☐ | |
| 30 | `dms add-member` | ☐ | Needs messages:write |
| 31 | `users get` | ☐ | Self, single, batch |
| 32 | `users set-profile` | ☐ | |
| 33 | `users presence` | ☐ | |
| 34 | `users set-presence` | ☐ | online, away, offline |
| 35 | `workflows list` | ☐ | |
| 36 | `workflows create` | ☐ | |
| 37 | `workflows update` | ☐ | |
| 38 | `workflows delete` | ☐ | |
| 39 | `workflows trigger` | ☐ | |
| 40 | `workflows runs` | ☐ | |
| 41 | `workflows get` | ☐ | |
| 42 | `workflows approve` | ☐ | Validation only (needs approval gate); bare = approve, `--approved false` = deny |
| 43 | `feed get` | ☐ | |
| 44 | `social publish` | ☐ | |
| 45 | `social set-contacts` | ☐ | |
| 46 | `social event` | ☐ | |
| 47 | `social notes` | ☐ | |
| 48 | `social contacts` | ☐ | |
| 49 | `repos create` | ☐ | |
| 50 | `repos get` | ☐ | |
| 51 | `repos list` | ☐ | |
| 52 | `repos protect list` | ☐ | Empty/populated rules; unknown rules visible; malformed rule reported in validation_error |
| 53 | `repos protect set` | ☐ | Create and replace complete exact-ref rule; verify metadata is preserved |
| 54 | `repos protect remove` | ☐ | Remove exact ref; missing rule → NotFound |
| 55 | `upload file` | ☐ | |
| 56 | `pack validate` | ☐ | Local, no relay |
| 57 | `pack inspect` | ☐ | Local, no relay |
| 58 | `notes set` | ☐ | First publish, edit/carry, --clear-tags, ambiguity, empty-stdin guard |
| 59 | `notes get` | ☐ | By name, by naddr, --content-only, cross-author, ambiguous → exit 1 |
| 60 | `notes ls` | ☐ | Own, --author all, --tag, --limit |
| 61 | `notes rm` | ☐ | Delete→get 404, double-delete idempotent, missing slug → NotFound |
| 62 | `users set-status` | ☐ | Text+emoji, text only, emoji-only (`--text ""`), `--clear`, `--clear` + `--text` → exit 1 |
