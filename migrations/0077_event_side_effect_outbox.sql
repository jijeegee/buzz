-- Side-effect outbox: a stored event whose kind has side effects gets a
-- pending row in the same transaction as the event insert.
--
-- The relay runs the side effects right after the insert commits and deletes
-- the row on success. A row that stays (the handler failed, or the relay
-- stopped in between) is retried by a background worker once its lease
-- expires, and by a duplicate re-send of the same event. Leases keep two
-- relays from running one row at the same time.
--
-- A trigger (not each insert path) writes the row, so every way an event is
-- stored (plain insert, replaceable, parameterized replaceable, channel head)
-- is covered. The kind list mirrors `is_side_effect_kind` in
-- crates/kuzz-relay/src/handlers/side_effects.rs.
CREATE TABLE event_side_effects (
    community_id UUID NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    event_id BYTEA NOT NULL,
    kind INTEGER NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    lease_until TIMESTAMPTZ NOT NULL DEFAULT now() + interval '60 seconds',
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    PRIMARY KEY (community_id, event_id)
);

CREATE INDEX event_side_effects_due ON event_side_effects (lease_until);
SELECT attach_community_write_fence('event_side_effects');

CREATE FUNCTION enqueue_event_side_effect() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.kind IN (0, 5, 10100, 30617, 40099)
       OR NEW.kind BETWEEN 9000 AND 9022
       OR NEW.kind BETWEEN 41001 AND 41003 THEN
        INSERT INTO event_side_effects (community_id, event_id, kind)
        VALUES (NEW.community_id, NEW.id, NEW.kind)
        ON CONFLICT DO NOTHING;
    END IF;
    RETURN NULL;
END
$$;

CREATE TRIGGER enqueue_event_side_effect
AFTER INSERT ON events
FOR EACH ROW EXECUTE FUNCTION enqueue_event_side_effect();
