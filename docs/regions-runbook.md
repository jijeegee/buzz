# Regions runbook

Steps for operators. How regions work is in [regions.md](regions.md).

One region is the **primary** (today `kr`). It holds the global lists
(accounts, communities, members, bans, the room directory, the region list)
and copies them to every other region. Every other region holds a **copy**,
plus the rooms whose home it is.

## Before adding a region

- **Postgres.** The new region's database is empty and runs the same
  Postgres major version as the primary's.
  - The primary's Postgres runs with `wal_level=logical`, and has enough
    `max_replication_slots` and `max_wal_senders` for one more region.
- **Network.** The new region's database can reach the primary's database
  over TLS.
  - Use a role with the `REPLICATION` attribute and `SELECT` on the
    published tables only.
  - Allow it in the primary's `pg_hba.conf`, for that address only.
- **Keys.**
  - `KUZZ_RELAY_PRIVATE_KEY` is the **same** in every region. It is the relay's identity, and apps check events it signs.
  - `KUZZ_REGION_KEY` is **new** for each region (its link key). Never reuse one;
    `regions add` refuses a key another region has.
- **Name.** Give the region its own address, for example `jp.<service domain>`.
  - With Cloudflare, add a DNS record for it, proxied, pointing at the region's load balancer.
  - Apps reach a region only through the address in its `connect_url`. The service name the apps sign for does not change (stage S1).
- **Media.** All regions share one bucket. While more than one region exists, the
  cleanup of unused uploads pauses (`kuzz_media_gc_paused_total`) until
  cleanup across regions is built (S6-2).

## Adding a region

1. **Start the relay** on the empty database, with:

   | Setting | Value |
   |---------|-------|
   | `KUZZ_REGION_ID` | the new id, e.g. `jp` |
   | `KUZZ_PRIMARY_REGION` | the primary's id |
   | `KUZZ_REGION_CONNECT_URL` | the region's address (`wss://jp.…`) |
   | `KUZZ_REGION_LINK` | `on` |
   | `KUZZ_REGION_KEY` | the region's new link key |
   | `KUZZ_COMMUNITY_ID` | the same as in the primary |
   | `KUZZ_RELAY_PRIVATE_KEY` | the same as in the primary |
   | `RELAY_URL` | the same service name as in the primary |
   | `KUZZ_AUTO_MIGRATE` | `true` (or run `kuzz-admin migrate` first) |

   `KUZZ_GLOBAL_COPY` turns on by itself. The relay migrates the database and then waits.
   The subscription in step 2 needs the tables, so migrate before it.

2. **Subscribe to the copy** in the new region's database:

   ```sql
   CREATE SUBSCRIPTION kuzz_global
     CONNECTION 'host=<primary db> dbname=kuzz user=<replication role> sslmode=verify-full …'
     PUBLICATION kuzz_global;
   ```

   The relay logs that it is waiting for the first copy. That takes as long as
   copying the accounts and lists; rooms are not copied.

3. **Add the region at the primary.** The waiting relay logs its id and
   `link_pubkey`. In the primary's relay container (`DATABASE_URL` is the
   primary's database):

   ```sh
   kuzz-admin regions add --id jp --connect-url wss://jp.example --link-pubkey <link_pubkey>
   ```

   The row reaches the new region through the copy, and its relay starts.

4. **Check:**
   - `kuzz-admin regions list` at the primary shows the region.
   - The new relay's `/health` answers.
   - `kuzz_global_copy_lag_seconds` in the new region stays near 1.
   - The new relay's log has no "is written in the primary region" errors.
   - Sign in through the new address, open a room, send a message.

Running `regions add` again with the same values changes nothing. With a new address
or key it updates the row, which is how you move a region's address or replace a leaked link
key. Restart that region's relay with the new key after the row has reached it.

## Changing the service name

When the service name changes (S1), change `communities.host` in the primary
together with `RELAY_URL` in every region. Apps sign for the service name,
so a mismatch refuses every sign-in.

## Removing a region

1. Move its rooms to other regions (S6-3/S6-4). `kuzz-admin regions list`
   shows how many rooms each region holds.
2. Stop the region's relays. Its apps reconnect through other addresses.
3. `kuzz-admin regions remove --id jp` at the primary. It refuses while any
   room's home is the region, and homes the region's accounts in the primary.
4. In the region's database, `DROP SUBSCRIPTION kuzz_global`. That frees the
   primary's replication slot; a slot left behind keeps WAL on the primary
   until its disk fills.
5. Remove the region's DNS record.

## Region commands refuse outside the primary

`kuzz-admin regions add|remove` run only against the primary region's
database. In a copy region they answer "not the primary": the list is written
in one place and copied everywhere.

## Turning stages off

Each stage of the regions work can be turned off by a setting, except where
noted. Migrations stay in place; none needs reverting. Settings are per relay
and take effect on restart.

| Stage | Turn off with | What stays, and is harmless |
|-------|---------------|-----------------------------|
| S0 order numbers | `KUZZ_PUBSUB_SEQ_ENVELOPE=off` | 0075 (no wasted numbers on duplicates), 0082, 0083 (event order per room). Pods read both message forms, so pods can be switched one by one. |
| S0 resync signal | `KUZZ_RESYNC_SIGNAL=off` | Nothing; the relay only counts lag. |
| S0 side-effect retry | `KUZZ_SIDE_EFFECT_RETRY=off` | 0077, 0081: `event_side_effects` keeps getting a row per event and deletes it on success. A failed side effect is not retried and its row stays (see below). |
| S1 service names | `KUZZ_SERVICE_ALIASES=` (empty) | Nothing. Apps that connect through an alias stop reaching the community. |
| S2 home markers | cannot be turned off | 0078–0080: one small directory row per room and a home region per account. Nothing reads them while there is one region. |
| S3 routing | no setting: active while `regions` has more than one row | No migrations. With one region nothing is looked up. |
| S4 region link | `KUZZ_REGION_LINK=off` | 0084–0086. Rows left in `region_outbox` only close connections and can be deleted. `user_block_versions` rows are tombstones. |
| S5 global copy | only with the link (see below) | 0087: the guard triggers stay on the copy's tables. |

**Apps.** Apps that opted into order numbers (`?kuzz_seq=1`) keep getting
them from history and `OK` frames. With the envelope off, a live message
that reaches a pod from another pod comes without a number; with the resync
signal off, apps learn of a gap only when they reconnect.

**Side effects with the retry off.** Watch for rows that stay:

```sql
SELECT count(*), min(created_at) FROM event_side_effects WHERE lease_until < now();
```

Turn the retry back on to run them, or delete them after checking `last_error`.

### Taking back a second region

A region other than the primary holds the global copy, and **cannot run with
the link off**:
- its guard triggers refuse the writes a relay without the link makes at startup;
- its rooms are recorded in the primary as living there.

So the way back is to remove the region, not to turn its link off:

1. Move its rooms to the primary or another region (S6-3/S6-4).
2. Follow [Removing a region](#removing-a-region).

While no room's home is the region, step 1 is empty and taking it back is quick.

The primary keeps running with its link off. In that case:
- it stops its heartbeat, so every copy region's lag grows, and copy regions then refuse ban and membership checks they cannot confirm;
- the publication and the full replica identity on `events` stay, and so does the extra WAL they cost;
- each copy region's subscription keeps its slot, and so keeps WAL on the primary, until that region drops it.

Turn the link off in the primary only once no other region is left.

### Not covered by tests

The two-region tests cover these:
- a link failing and coming back (`a_link_down_fails_retryably_and_never_splits_the_room`);
- a copy that stops and restarts (`e2e_regions_lag.rs`).

They do not cover:
- restarting a relay while messages flow (planned with the tangle tests, S4-8);
- turning the stages above off in a running deployment.

Try a switch in a staging deployment first.
