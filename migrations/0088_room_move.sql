-- Moving a room to another region (global regions S6-3).
--
-- `moving_since` freezes a room: its events are refused while the room is
-- copied to its new home, and for good in the old home once it has moved
-- (`moved_out_at`). The check sits in the sequence trigger, under the room's
-- event lock, which freezing takes too: events under way when the room is
-- frozen finish first, and every later one is refused. `moved_in_at` marks a
-- copy written by a move and not opened yet; only such a copy may be removed
-- when a move is abandoned.
--
-- Redefines the latest body from 0075_channel_sequence_skip_duplicates.sql.
ALTER TABLE channels
    ADD COLUMN moving_since TIMESTAMPTZ,
    ADD COLUMN moved_out_at TIMESTAMPTZ,
    ADD COLUMN moved_in_at TIMESTAMPTZ,
    ADD CONSTRAINT chk_channels_moved_out_frozen
        CHECK (moved_out_at IS NULL OR moving_since IS NOT NULL),
    ADD CONSTRAINT chk_channels_moved_in_frozen
        CHECK (moved_in_at IS NULL OR moving_since IS NOT NULL);

CREATE OR REPLACE FUNCTION stamp_channel_event_sequence() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
DECLARE
    frozen TIMESTAMPTZ;
    gone TIMESTAMPTZ;
BEGIN
    IF NEW.channel_id IS NULL THEN
        NEW.channel_sequence := 0;
        RETURN NEW;
    END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended(
        'kuzz_channel_membership:' || NEW.community_id::text || ':' || NEW.channel_id::text, 0));
    -- A room moving to another region (0088) takes no events, retried ones
    -- included. While its side effects drain, the relay's room messages and
    -- group snapshots they write still land, so the drain can finish.
    SELECT moving_since, moved_out_at INTO frozen, gone FROM channels
    WHERE community_id = NEW.community_id AND id = NEW.channel_id;
    IF frozen IS NOT NULL AND (
        gone IS NOT NULL
        OR NEW.kind NOT IN (39000, 39001, 39002, 39003, 40099)
        OR NOT EXISTS (
            SELECT 1 FROM event_side_effects s
            JOIN events e ON e.community_id = s.community_id AND e.id = s.event_id
            WHERE s.community_id = NEW.community_id AND e.channel_id = NEW.channel_id
        )
    ) THEN
        RAISE EXCEPTION 'room % is moving to another region', NEW.channel_id
            USING ERRCODE = 'KZ001';
    END IF;
    -- A retried event: the insert will hit ON CONFLICT DO NOTHING.
    IF EXISTS (
        SELECT 1 FROM events
        WHERE community_id = NEW.community_id
          AND created_at = NEW.created_at
          AND id = NEW.id
    ) THEN
        RETURN NEW;
    END IF;
    UPDATE channels SET accepted_sequence = accepted_sequence + 1
    WHERE community_id = NEW.community_id AND id = NEW.channel_id
    RETURNING accepted_sequence INTO NEW.channel_sequence;
    -- Existing event storage permits references to absent channels. They have
    -- no membership and cannot pass a reader's channel authorization.
    IF NOT FOUND THEN NEW.channel_sequence := 0; END IF;
    RETURN NEW;
END
$$;
