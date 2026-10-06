-- One channel ordering boundary for accepted events and current memberships.
-- Existing rooms retain all-history. No client timestamp grants visibility.
ALTER TABLE channels
    ADD COLUMN history_policy TEXT NOT NULL DEFAULT 'all'
        CHECK (history_policy IN ('all', 'since_join')),
    ADD COLUMN accepted_sequence BIGINT NOT NULL DEFAULT 0
        CHECK (accepted_sequence >= 0);
ALTER TABLE channels ALTER COLUMN history_policy SET DEFAULT 'since_join';

ALTER TABLE events ADD COLUMN channel_sequence BIGINT NOT NULL DEFAULT 0
    CHECK (channel_sequence >= 0);
ALTER TABLE channel_members
    ADD COLUMN join_sequence BIGINT NOT NULL DEFAULT 0 CHECK (join_sequence >= 0),
    ADD COLUMN join_cutoff_is_legacy BOOLEAN NOT NULL DEFAULT false;

-- Keep signed created_at untouched. received_at is the legacy server receipt
-- evidence; id/created_at provide deterministic ties, not client authority.
WITH ordered AS (
    SELECT community_id, created_at, id,
           row_number() OVER (
               PARTITION BY community_id, channel_id
               ORDER BY received_at, id, created_at
           ) AS sequence
    FROM events WHERE channel_id IS NOT NULL
)
UPDATE events e SET channel_sequence = ordered.sequence
FROM ordered
WHERE e.community_id = ordered.community_id
  AND e.created_at = ordered.created_at AND e.id = ordered.id;

UPDATE channels c SET accepted_sequence = COALESCE((
    SELECT max(e.channel_sequence) FROM events e
    WHERE e.community_id = c.community_id AND e.channel_id = c.id
), 0);

-- Legacy reactivation did not update joined_at, and erased removed_at. We
-- cannot reconstruct those current membership periods. Preserve all-history
-- until an explicit policy change; disclose this conservative cutoff in UI.
UPDATE channel_members m SET
    join_sequence = c.accepted_sequence,
    join_cutoff_is_legacy = true
FROM channels c
WHERE c.community_id = m.community_id AND c.id = m.channel_id;

CREATE FUNCTION stamp_channel_event_sequence() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.channel_id IS NULL THEN
        NEW.channel_sequence := 0;
        RETURN NEW;
    END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended(
        'buzz_channel_membership:' || NEW.community_id::text || ':' || NEW.channel_id::text, 0));
    UPDATE channels SET accepted_sequence = accepted_sequence + 1
    WHERE community_id = NEW.community_id AND id = NEW.channel_id
    RETURNING accepted_sequence INTO NEW.channel_sequence;
    -- Existing event storage permits references to absent channels. They have
    -- no membership and cannot pass a reader's channel authorization.
    IF NOT FOUND THEN NEW.channel_sequence := 0; END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER stamp_channel_event_sequence
BEFORE INSERT ON events
FOR EACH ROW EXECUTE FUNCTION stamp_channel_event_sequence();

CREATE FUNCTION stamp_channel_membership_cutoff() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE' THEN
        IF OLD.community_id IS DISTINCT FROM NEW.community_id
           OR OLD.channel_id IS DISTINCT FROM NEW.channel_id
           OR OLD.pubkey IS DISTINCT FROM NEW.pubkey THEN
            RAISE EXCEPTION 'channel membership identity is immutable'
                USING ERRCODE = 'check_violation';
        END IF;
        -- Active duplicate joins and role changes never advance the boundary.
        IF OLD.removed_at IS NULL OR NEW.removed_at IS NOT NULL THEN
            NEW.join_sequence := OLD.join_sequence;
            NEW.joined_at := OLD.joined_at;
            NEW.join_cutoff_is_legacy := OLD.join_cutoff_is_legacy;
            RETURN NEW;
        END IF;
    END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended(
        'buzz_channel_membership:' || NEW.community_id::text || ':' || NEW.channel_id::text, 0));
    SELECT accepted_sequence INTO STRICT NEW.join_sequence FROM channels
    WHERE community_id = NEW.community_id AND id = NEW.channel_id;
    NEW.joined_at := clock_timestamp();
    NEW.join_cutoff_is_legacy := false;
    RETURN NEW;
END
$$;

CREATE TRIGGER stamp_channel_membership_cutoff
BEFORE INSERT OR UPDATE ON channel_members
FOR EACH ROW EXECUTE FUNCTION stamp_channel_membership_cutoff();
