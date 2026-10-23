# Regions

Kuzz is moving from one server to several regions (for example `kr`, `jp`,
`sg`, `us`) that serve one service. This page records what exists today and
the rules later stages must follow. Production runs a single region; nothing
below changes single-region behaviour.

## Region of a relay

| Setting | Default | Meaning |
|---------|---------|---------|
| `KUZZ_REGION_ID` | `kr` | This relay's region. A lowercase letter, then up to 15 lowercase letters, digits or `-`. |
| `KUZZ_PRIMARY_REGION` | this region | The region that runs deployment-wide jobs and holds the global lists. |
| `KUZZ_REGION_CONNECT_URL` | `RELAY_URL` | Address clients in this region connect to. |

At startup the relay:

1. refuses to start when the database already belongs to another region
   (`local_region`);
2. records its row in `regions` (at most one row is primary; a second
   primary is refused);
3. claims the database for its region;
4. gives every room without a `room_directory` row, and every account
   without `principals.home_region`, the local region.

## Global copy

Global data is written only in the primary region and copied to every other
region by Postgres logical replication (stage S5). The table lists live in
`crates/kuzz-db/src/store/global_copy.rs`: every table is either copied or
local, and a test fails when a new table is in neither list.

- **Copied:** regions, the room directory, communities, members, invites,
  bans, moderation, accounts and sign-in (principals, identities, devices,
  sessions, tokens, bots, key backups), profiles (`users`), push leases, git
  repository names, and the room-less rows of `events` (profiles, contact
  lists, settings, agent records).
- **Local:** rooms and their messages, blocks (S4-5b writes them in every
  region at once), work queues and logs, `local_region`.

| Setting | Default | Meaning |
|---------|---------|---------|
| `KUZZ_GLOBAL_COPY` | on for a region other than the primary with `KUZZ_REGION_LINK=on` | This database's global tables are a copy. `off` keeps a region's own global tables (development only). |

The primary (with the link on) creates the publication `kuzz_global` at
startup and gives `events` the full replica identity its row filter needs.
It updates `global_heartbeat` every second.

Adding a region:

1. Its Postgres runs the same version; the primary's runs with
   `wal_level=logical` and lets the new region's database connect for
   replication.
2. Start the new region's relay on an empty database. It migrates, then
   waits.
3. In the new database: `CREATE SUBSCRIPTION kuzz_global CONNECTION '…the
   primary…' PUBLICATION kuzz_global`. The relay waits until the first copy
   of every table is done.
4. In the primary: `INSERT INTO regions (id, connect_url, link_pubkey)` with
   the values the waiting relay logs. The row reaches the new region by the
   copy, and the relay starts.

From then on the relay refuses local writes to the copied tables (a trigger;
replication itself is not affected) and exports
`kuzz_global_copy_lag_seconds`, the age of the newest heartbeat it holds.
A version that copies more tables needs `ALTER SUBSCRIPTION kuzz_global
REFRESH PUBLICATION` in each region; the relay waits and says so until then.

A migration that writes rows into a copied table must skip copies
(`WHERE NOT kuzz_is_global_copy()`).

## What records ownership

| Table / column | Scope | Written by |
|----------------|-------|------------|
| `regions` | deployment, copied | the primary's relay at startup (its own row); other regions are added in the primary |
| `local_region` | this database only, never copied | first relay start |
| `room_directory` (`home_region`, `epoch`, DM `participant_hash`) | per community, copied | trigger on `channels` insert; startup backfill |
| `principals.home_region` | deployment | trigger on `principals` insert; startup backfill |
| `principals.country` | deployment | OIDC sign-up, from `CF-IPCountry` via a trusted proxy only |

Nothing reads these yet. The routing layer (stage S3) will send each room
operation to its home region.

## Background jobs by region

Before a second region is turned on, every job started from
`crates/kuzz-relay/src/main.rs` must run in the scope below.

- **every**: each relay process, for local state (sockets, caches, its own
  database).
- **home**: each region, only for rooms that region owns.
- **primary**: the primary region only.

| Job | Scope | Why |
|-----|-------|-----|
| Redis subscribers and consumers: fan-out, cache invalidation, connection control, NIP-FI disconnect, resync, token revocation | every | they act on local sockets and caches |
| read-pool ping, replica fence probe, pool metrics, dependency sampler, health listener | every | local process |
| partition audit, periodic `ANALYZE events` | every | each region has its own database |
| community revalidator | every | local sockets |
| NIP-FI JWKS refresh | every | local cache |
| mesh boot (`KUZZ_MESH`) | every | off in production |
| NIP-43 membership snapshot reconcile | home | membership lives with the room |
| channel discovery reconcile (`KUZZ_RECONCILE_CHANNELS`, dev/CI) | home | channel rows live in the home region |
| ephemeral channel reaper | home | channel rows live in the home region |
| side-effect retry (`event_side_effects`) | home | the outbox row is written with the event |
| push matcher | home | matching needs the stored event |
| NIP-ER reminder scheduler | home | reminders belong to a room |
| workflow cron | home | workflows belong to a room |
| audit writer | every | each region's database holds its own per-community chains |
| push delivery worker | primary | push registrations are global |
| operator-listener reaper and delivery | primary | operator state is global |
| admin outbox worker, admin action recovery | primary | admin actions are global |
| usage metrics leader (invite reaper, storage sweep) | primary | deployment-wide counts |
| media GC | primary, checking every region's references | a file can be referenced from any region; deleting on one region's view would remove files other regions use |

Jobs that already coordinate through database leases, advisory locks or a
leader keep doing so within their scope.

## Audit chains

`audit_log` lives in each region's own database, so every region keeps its
own chain per community and two regions never append to the same chain. The
region id is not part of the hash. Add it (a new `hash_version`) before audit
rows from several regions are ever combined in one place, so an entry cannot
be moved from one region's chain to another's unnoticed.
