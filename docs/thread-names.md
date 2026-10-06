# Shared thread names

Open a stream thread and use the pencil beside its heading to set or change its
name. The name also appears in the channel reply summary and sidebar activity
preview. Save an empty name to restore the normal heading. Other channel readers
receive changes live. Message bodies and reply counts do not change.

The limit is 40 units: ASCII characters (including spaces) cost one unit and
other Unicode scalars cost two. This permits 40 English or 20 composed Korean
characters, with the same budget for mixed names. Names are single-line, trimmed,
and contain no control characters. Desktop normalizes input to NFC before saving.
Korean IME Enter does not submit the form while composition is in progress.

## Event contract

Kind `40009` (`KIND_THREAD_NAME`) has content equal to the name and exactly one
plain `h` tag (channel UUID) and one plain `e` tag (lowercase thread head ID).
Empty content clears the name. It is a regular channel-scoped event, with the
existing channel write-access, tenant and moderation gates. The referenced head
must be a stream message in that same channel; nested stream branches are allowed.
Thread name events never acquire reply ancestry or increment reply counters.

Readers select the latest event by descending `created_at`, then ascending event
ID, matching relay query order. Writers read the current head and use at least
its timestamp plus one second, so sequential saves within a second take effect.
Concurrent writes use the same deterministic ordering; this is last-write-wins,
not a compare-and-swap operation. History is preserved in the event store.

Agents can use:

```sh
buzz messages name-thread --channel <UUID> --event <HEAD_ID> --name "Thread naming"
buzz messages name-thread --channel <UUID> --event <HEAD_ID> --name ""
```

Deploy the updated relay before clients, because older relays reject the new
kind. No database migration is required. This change adds desktop UI and CLI;
mobile clients continue to display their existing headings.

## Validation

- Core tests cover language boundaries and malformed/marked references.
- The ignored relay `thread_name_round_trip_preserves_reply_counts_and_channel_access`
  test requires an isolated Postgres database initialized with `schema/schema.sql`.
- Desktop `threadName.test.mjs` covers ordering, clear events and channel isolation.
- Playwright `thread-name.spec.ts` exercises editing, limits, Korean composition,
  list display, live updates and clearing through the application.
