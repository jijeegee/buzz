# Goal store cutover

One-off move of goals from the kind 40110 document to the `goals` and
`goal_links` tables. It is not a migration: it runs once, at the switch, on the
last heads. Run it on a copy first.

The relay must already be on an image with migration 0099, with
`KUZZ_GOAL_STORE` off. Writes must be stopped and the flag on before the export
below, so the heads are final.

## Import

Postgres runs in Docker with no published port, so `psql` goes through the
container, and the server has no Rust toolchain, so `goal_import` runs on a PC.
Use Git Bash there: PowerShell has no `<`, and its pipes can turn Korean into `?`.
`$PSQL` below is, on the server, in `/opt/kuzz`:

```sh
PSQL="docker compose exec -T postgres psql -U $PGUSER -d $PGDATABASE -v ON_ERROR_STOP=1"
```

On the server:

```sh
$PSQL -At < export_heads.sql > heads.jsonl
wc -l heads.jsonl  # one line per room with goals
```

On the PC, after copying `heads.jsonl` over:

```sh
set -e
cargo test -q -p kuzz-core --example goal_import
cargo run -q -p kuzz-core --example goal_import < heads.jsonl > import.sql
```

CI compiles this example but doesn't run its tests, hence the first line. Its
stderr lists goals kept and left out per room. Read `import.sql`, copy it back,
and on the server:

```sh
$PSQL -1 < import.sql
```

`-1` makes the import all or none.

What the import does:

- Leaves out what the deletion rule would have removed, using the relay's own
  rule in `kuzz_core::goal_store`, including unfinished branches under a
  finished thread goal.
- Drops links on layer 1.
- Keeps `updated_at` when a goal has it; otherwise uses the head's time, never
  `now()`, so nothing looks recently touched. A goal takes the time of a newer
  task left out below it, as the relay stamps on delete. `created_at` takes the
  same value.

`import.sql` starts by emptying `goals`, so running it again replaces them.
40110 events are not touched; they stay readable.

## Reclassify

Once, right after the import, on the Kuzz room:

```sh
$PSQL -1 < reclassify_kuzz_room.sql
```

It stops without changing anything if the room's layer 2 goals no longer match
its list. Update the list first when a layer 2 goal was added, removed or moved.
Run it after a re-import too, because the import puts the room back as it was.

## Rollback

Before app updates open: turn `KUZZ_GOAL_STORE` off on the same image. Do not
change the image, because older images refuse to start after 0099. 40110 writes
work again and anything written as 40111 since the switch is lost. To switch
again, re-export and re-import.

After app updates open, new apps have no 40110 writes, so fix forward.
