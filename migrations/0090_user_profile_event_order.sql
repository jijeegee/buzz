-- Which profile event the users row shows (replay safety, before S4).
--
-- kind:0 (profile) and kind:10100 (agent profile) are replaceable events. Their
-- side effects copy fields into `users`. A retry or a forwarded copy of an
-- older event could run after a newer one and bring back stale fields. Each
-- side effect now records the event it applied and skips an event that NIP-01
-- would not let replace it: an older created_at, or the same created_at with a
-- larger id. A re-run of the same event is skipped too.
--
-- NULL means no event recorded yet (a row written without an event); the
-- next event applies.
ALTER TABLE users
    ADD COLUMN profile_event_at       TIMESTAMPTZ,
    ADD COLUMN profile_event_id       BYTEA,
    ADD COLUMN add_policy_event_at    TIMESTAMPTZ,
    ADD COLUMN add_policy_event_id    BYTEA;

-- Existing rows record the newest stored event whose side effect finished
-- (no outbox row left). Profile side effects are retried from now on, so an
-- older failed one must not apply over the current fields; a newer failed one
-- still applies when retried.
UPDATE users u
   SET profile_event_at = e.created_at, profile_event_id = e.id
  FROM (SELECT DISTINCT ON (ev.community_id, ev.pubkey)
               ev.community_id, ev.pubkey, ev.created_at, ev.id
          FROM events ev
         WHERE ev.kind = 0 AND ev.deleted_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM event_side_effects o
                            WHERE o.community_id = ev.community_id AND o.event_id = ev.id)
         ORDER BY ev.community_id, ev.pubkey, ev.created_at DESC, ev.id ASC) e
 WHERE u.community_id = e.community_id AND u.pubkey = e.pubkey;

UPDATE users u
   SET add_policy_event_at = e.created_at, add_policy_event_id = e.id
  FROM (SELECT DISTINCT ON (ev.community_id, ev.pubkey)
               ev.community_id, ev.pubkey, ev.created_at, ev.id
          FROM events ev
         WHERE ev.kind = 10100 AND ev.deleted_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM event_side_effects o
                            WHERE o.community_id = ev.community_id AND o.event_id = ev.id)
         ORDER BY ev.community_id, ev.pubkey, ev.created_at DESC, ev.id ASC) e
 WHERE u.community_id = e.community_id AND u.pubkey = e.pubkey;
