-- Moving a room to another region (global regions S6-3).
--
-- `moving_since` freezes a room: its events are refused while the room is
-- copied to its new home, and for good in the old home once it has moved
-- (`moved_out_at`). The check sits in the sequence trigger, which already
-- takes the room's row lock for every event: writes under way when the room
-- is frozen finish first, and every later one is refused.
--
-- Redefines the latest body from 0075_channel_sequence_skip_duplicates.sql.
ALTER TABLE channels
    ADD COLUMN moving_since TIMESTAMPTZ,
    ADD COLUMN moved_out_at TIMESTAMPTZ,
    ADD CONSTRAINT chk_channels_moved_out_frozen
        CHECK (moved_out_at IS NULL OR moving_since IS NOT NULL);

CREATE OR REPLACE FUNCTION stamp_channel_event_sequence() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
DECLARE
    frozen TIMESTAMPTZ;
BEGIN
    IF NEW.channel_id IS NULL THEN
        NEW.channel_sequence := 0;
        RETURN NEW;
    END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended(
        'kuzz_channel_membership:' || NEW.community_id::text || ':' || NEW.channel_id::text, 0));
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
    RETURNING accepted_sequence, moving_since INTO NEW.channel_sequence, frozen;
    -- Existing event storage permits references to absent channels. They have
    -- no membership and cannot pass a reader's channel authorization.
    IF NOT FOUND THEN NEW.channel_sequence := 0; END IF;
    -- A room moving to another region (0088) takes no events.
    IF frozen IS NOT NULL THEN
        RAISE EXCEPTION 'room % is moving to another region', NEW.channel_id
            USING ERRCODE = 'KZ001';
    END IF;
    RETURN NEW;
END
$$;
