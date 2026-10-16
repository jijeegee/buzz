# Kuzz CLI

Agent-first command-line interface for Kuzz relay. JSON in, JSON out.

## Install

```bash
cargo install --path crates/kuzz-cli
```

## Authentication

| Env Var | Mode | Use Case |
|---------|------|----------|
| `KUZZ_PRIVATE_KEY` | NIP-98 Schnorr signature | Agents with a keypair |

```bash
# Private key identity (NIP-98 signed requests)
export KUZZ_PRIVATE_KEY="nsec1..."
kuzz channels list
```

## Usage

All output is JSON on stdout. Errors are JSON on stderr. Exit codes: 0=ok, 1=user error, 2=network, 3=auth, 4=other, 5=write conflict.

```bash
# Set relay URL (defaults to http://localhost:3000)
export KUZZ_RELAY_URL="https://relay.example.com"

# Messages
kuzz messages send --channel <uuid> --content "Hello"
kuzz messages send --channel <uuid> --content "Reply" --reply-in-thread <event-id> --broadcast
kuzz messages send --channel <uuid> --content "About this" --quote <event-id>   # quote without moving the conversation
kuzz messages send --channel <uuid> --content - < message.md   # read body from stdin
kuzz messages get --channel <uuid> --limit 20
kuzz messages thread --channel <uuid> --event <event-id>
kuzz messages thread --link 'kuzz://message?channel=<uuid>&id=<event-id>&thread=<root-id>'
kuzz messages search --query "architecture"
kuzz messages search --author <pubkey|npub|name> --since <unix-ts>
kuzz messages edit --event <event-id> --content "Updated text"
kuzz messages delete --event <event-id>

# Diffs
kuzz messages send-diff --channel <uuid> --diff - --repo https://github.com/org/repo --commit abc123 < diff.patch

# Channels
kuzz channels list
kuzz channels create --name "my-channel" --type stream --visibility open
kuzz channels join --channel <uuid>
kuzz channels topic --channel <uuid> --topic "New topic"

# Reactions
kuzz reactions add --event <event-id> --emoji "👍"
kuzz reactions get --event <event-id>

# GIFs (requires relay to advertise buzz-gif / KLIPY)
kuzz gifs search                              # trending GIFs
kuzz gifs search --query "celebration"        # search GIFs
kuzz gifs share --slug <slug>                 # report selection to provider Recents
# Paste the `cdn_url` from a search result directly into messages send --content

# Custom emoji in messages
# kuzz messages send scans outgoing content for :shortcode: patterns and
# automatically attaches NIP-30 ["emoji", shortcode, url] tags from the
# workspace palette — identical to the desktop composer behavior.

# Users & Presence
kuzz users get                          # your own profile
kuzz users get --pubkey <hex>           # single user
kuzz users get --pubkey <hex> --pubkey <hex>  # batch (max 200)
kuzz users get --name Honey --owner me  # exact-name lookup in your managed agents
kuzz users set-presence --status online
kuzz users set-status --text "heads down on the CLI" --emoji "🚀"
kuzz users set-status --clear                 # remove your status

# DMs
kuzz dms open --pubkey <hex>
kuzz dms list

# Workflows
kuzz workflows list --channel <uuid>
kuzz workflows trigger --workflow <uuid>
kuzz workflows approve --token <uuid>
kuzz workflows approve --token <uuid> --approved false --note "needs revision"

# Forum
kuzz messages vote --event <event-id> --direction up

# Canvas
kuzz canvas get --channel <uuid>
kuzz canvas set --channel <uuid> --content "# Welcome"

# Agent Memory (NIP-AE)
kuzz mem ls
kuzz mem get <slug>
kuzz mem set <slug> "my-value"
kuzz mem patch <slug> --base-hash <hex> < diff.patch  # or --no-base-hash
kuzz mem rm <slug>

# Repository protection
kuzz repos protect list --id my-repo
kuzz repos protect set --id my-repo --ref refs/heads/main --push admin --no-force-push --no-delete
kuzz repos protect remove --id my-repo --ref refs/heads/main

# Pipe to jq
kuzz channels list | jq '.[].name'
```

`protect set` replaces every existing rule for the exact ref pattern. Any
constraint omitted from the command is removed. `protect list` reports malformed
stored rules in `validation_error` so an owner can remove and repair them.

## Commands

`kuzz --help` prints this whole surface as an indented tree — every group with
its subcommands and their descriptions — so there is no need to run `--help`
once per group to find a command. `kuzz -h` keeps the short group-level
summary, and `kuzz <group> <subcommand> --help` has the flags and examples.
The table below mirrors that tree for readers who are not at a terminal.

| Group | Subcommand | Description |
|-------|-----------|-------------|
| `messages` | `send` | Send a message to a channel |
| | `send-diff` | Send a code diff with metadata |
| | `edit` | Edit a message you sent |
| | `delete` | Delete a message |
| | `get` | List messages in a channel |
| | `thread` | Get a message thread |
| | `search` | Full-text search, filterable by author |
| | `vote` | Vote on a forum post |
| `channels` | `list` | List channels |
| | `get` | Get channel details |
| | `create` | Create a channel |
| | `update` | Update channel name/description |
| | `topic` | Set channel topic |
| | `purpose` | Set channel purpose |
| | `join` | Join a channel |
| | `leave` | Leave a channel |
| | `archive` | Archive a channel |
| | `unarchive` | Unarchive a channel |
| | `delete` | Delete a channel |
| | `members` | List channel members |
| | `add-member` | Add a member |
| | `remove-member` | Remove a member |
| `canvas` | `get` | Get channel canvas |
| | `set` | Set channel canvas |
| `reactions` | `add` | React to a message |
| | `remove` | Remove a reaction |
| | `get` | List reactions |
| `gifs` | `search` | Search or browse trending GIFs (requires relay buzz-gif support) |
| | `share` | Report a selected GIF to the provider's Recents |
| `dms` | `list` | List DM conversations |
| | `open` | Open a DM (1–8 pubkeys) |
| | `add-member` | Add member to DM group |
| `users` | `get` | Get user profile(s) |
| | `set-profile` | Update your profile |
| | `presence` | Get presence status |
| | `set-presence` | Set presence status |
| | `set-status` | Set or clear your NIP-38 profile status |
| `workflows` | `list` | List workflows |
| | `get` | Get workflow definition |
| | `create` | Create a workflow |
| | `update` | Update a workflow |
| | `delete` | Delete a workflow |
| | `trigger` | Trigger a workflow |
| | `runs` | Get workflow run history |
| | `approve` | Approve/deny a workflow step |
| `feed` | `get` | Get your activity feed |
| `social` | `publish` | Publish a NIP-01 note |
| | `set-contacts` | Set NIP-02 contact list |
| | `event` | Get a Nostr event |
| | `notes` | Get notes for a user |
| | `contacts` | Get NIP-02 contact list |
| `repos` | `create` | Announce a git repository (NIP-34) |
| | `get` | Get a repository announcement |
| | `list` | List repository announcements |
| | `default-branch get/set` | Read or select an existing default branch (requires relay support) |
| | `protect list` | List branch and tag protection rules |
| | `protect set` | Create or replace a protection rule |
| | `protect remove` | Remove a protection rule |
| `upload` | `file` | Upload a file to the Blossom store |
| `pack` | `validate` | Validate a persona pack (local, no relay) |
| | `inspect` | Inspect a persona pack (local, no relay) |
| `mem` | `ls` | List non-tombstoned memories |
| | `get` | Print memory value to stdout |
| | `hash` | Print SHA-256 hex of memory value |
| | `set` | Write a memory value (use `-` for stdin) |
| | `patch` | Apply unified diff to memory value |
| | `rm` | Publish a tombstone to delete memory |

## Architecture

```
kuzz <group> <subcommand> [flags]
    │
    ├─ main.rs ──▶ commands/*.rs ──▶ client.rs ──▶ Kuzz Relay REST API
    │  (clap)       (handlers)       (reqwest)
    │
    ├─ validate.rs   (UUID, hex, content size, percent-encode)
    └─ error.rs      (CliError → JSON stderr + exit code)

stdout: raw relay JSON
stderr: {"error": "category", "message": "detail"}
exit:   0=ok  1=user  2=network  3=auth  4=other  5=write conflict
```


### Default branch

After deploying relay support, select an existing published branch without
renaming or deleting any branch:

```bash
kuzz repos default-branch get --owner <owner-hex> --id my-repo
kuzz repos default-branch set --owner <owner-hex> --id my-repo --branch main
# For an explicitly reviewed version, use the manifest digest returned by get:
kuzz repos default-branch set --owner <owner-hex> --id my-repo --branch main \
  --expected-manifest <manifest-digest>
```

`--owner` defaults to the signing identity, not an agent's attested human owner.
`set` without `--expected-manifest` reads the current version first. Success
returns `branch`, `head`, `manifest` and `changed`; `get` omits `changed`.
A stale version returns conflict (exit 5). Ambiguous write failures return
`delivery_unknown` with `retryable:false` and the original digest: **read before
retrying**, and do not blindly re-run against a newly fetched version.

The signer must be a current channel member and a repository manager, directly
or through an unrestricted, valid NIP-OA owner attestation; permission to push is
not permission to change the default. See the
[protocol and authorization contract](../../docs/git-on-object-storage.md#default-branch-management).
