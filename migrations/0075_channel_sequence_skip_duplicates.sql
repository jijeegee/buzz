-- Duplicate event inserts no longer burn channel sequence numbers.
--
-- `stamp_channel_event_sequence` runs BEFORE INSERT, so a retried event that
-- later hits `ON CONFLICT DO NOTHING` still advanced `channels.accepted_sequence`
-- and left a hole in the channel's order. After taking the per-channel lock,
-- skip the increment when a row with the same primary key already exists; the
-- insert then conflicts and the original row keeps its number.
--
-- Redefines the latest body from 0067_kuzz_protocol_names.sql.
CREATE OR REPLACE FUNCTION stamp_channel_event_sequence() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
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
    RETURNING accepted_sequence INTO NEW.channel_sequence;
    -- Existing event storage permits references to absent channels. They have
    -- no membership and cannot pass a reader's channel authorization.
    IF NOT FOUND THEN NEW.channel_sequence := 0; END IF;
    RETURN NEW;
END
$$;
