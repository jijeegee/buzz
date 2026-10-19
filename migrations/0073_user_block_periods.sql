-- ── Block periods: blocks hide what was sent while blocked ───────────────────
-- A block hides the blocked person's messages in a 1:1 DM only for the time
-- the block was on. Messages from before the block stay visible; messages
-- sent while blocked stay hidden, also after unblocking; messages after
-- unblocking are normal. Group DMs (3+ people) and channels are not filtered
-- by the server (clients fold blocked people's messages there).
--
-- user_blocks stays the "blocked right now" table (block list API, live
-- delivery, push, ingest-time unread counts). user_block_periods keeps every
-- block's time span, open while the block is on. Read paths that look at
-- stored messages (history, chats list, inbox summary, thread summaries,
-- recounts) hide a message iff its author has a period with the reader that
-- covers the message's server receipt time (events.received_at, never the
-- sender-signed created_at).
--
-- Block and unblock write both tables in one transaction (kuzz-db blocks.rs).
CREATE TABLE IF NOT EXISTS user_block_periods (
    community_id UUID NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    blocker      BYTEA NOT NULL CHECK (length(blocker) = 32),
    blocked      BYTEA NOT NULL CHECK (length(blocked) = 32),
    started_at   TIMESTAMPTZ NOT NULL,
    ended_at     TIMESTAMPTZ,
    PRIMARY KEY (community_id, blocker, blocked, started_at),
    CONSTRAINT user_block_periods_not_self CHECK (blocker <> blocked),
    CONSTRAINT user_block_periods_ordered CHECK (ended_at IS NULL OR ended_at >= started_at)
);

-- At most one open period per pair. Reader filters look up by
-- (community, blocker[, blocked]), which the primary key serves.
CREATE UNIQUE INDEX IF NOT EXISTS user_block_periods_open
    ON user_block_periods (community_id, blocker, blocked) WHERE ended_at IS NULL;

SELECT attach_community_write_fence('user_block_periods');

-- Current blocks become open periods from the time they were made.
INSERT INTO user_block_periods (community_id, blocker, blocked, started_at)
SELECT ub.community_id, ub.blocker, ub.blocked, ub.created_at
  FROM user_blocks ub
 WHERE NOT EXISTS (
       SELECT 1 FROM user_block_periods bp
        WHERE bp.community_id = ub.community_id AND bp.blocker = ub.blocker
          AND bp.blocked = ub.blocked AND bp.ended_at IS NULL);

-- A DM between exactly two people. DM participant sets never change (a new
-- member makes a new DM), so the member row count is the participant count.
CREATE OR REPLACE FUNCTION dm_is_one_to_one(p_cid UUID, p_chid UUID)
RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT EXISTS (SELECT 1 FROM channels c
                    WHERE c.community_id = p_cid AND c.id = p_chid AND c.channel_type = 'dm')
       AND (SELECT count(*) FROM channel_members m
             WHERE m.community_id = p_cid AND m.channel_id = p_chid) = 2
$$;

-- Unread counts (migration 0072) with the period rule: in a 1:1 DM, messages
-- the reader received from someone during one of the reader's blocks of them
-- do not count.
CREATE OR REPLACE FUNCTION chat_room_unread(
    p_cid UUID, p_chid UUID, p_reader BYTEA, p_after BIGINT,
    OUT unread INTEGER, OUT mentions INTEGER)
LANGUAGE plpgsql STABLE AS $$
DECLARE
    v_dm BOOLEAN;
    v_reader_hex TEXT := encode(p_reader, 'hex');
BEGIN
    v_dm := dm_is_one_to_one(p_cid, p_chid);
    SELECT count(*) INTO unread FROM (
        SELECT 1 FROM events e
         WHERE e.community_id = p_cid AND e.channel_id = p_chid
           AND e.channel_sequence > COALESCE(p_after, 0)
           AND e.kind IN (9, 40002, 45001, 45003) AND e.deleted_at IS NULL
           AND e.pubkey <> p_reader
           AND NOT EXISTS (
               SELECT 1 FROM thread_metadata r
                WHERE r.community_id = p_cid AND r.event_id = e.id AND r.depth > 0
                  AND (NOT r.broadcast OR chat_is_thread_marker(e.tags, r.root_event_id)))
           AND NOT (v_dm AND EXISTS (
               SELECT 1 FROM user_block_periods bp
                WHERE bp.community_id = p_cid AND bp.blocker = p_reader
                  AND bp.blocked = e.pubkey AND bp.started_at <= e.received_at
                  AND (bp.ended_at IS NULL OR e.received_at < bp.ended_at)))
         LIMIT 100) n;
    SELECT count(*) INTO mentions FROM (
        SELECT 1 FROM events e
         WHERE e.community_id = p_cid AND e.channel_id = p_chid
           AND e.channel_sequence > COALESCE(p_after, 0)
           AND e.kind IN (9, 40002, 45001, 45003) AND e.deleted_at IS NULL
           AND e.pubkey <> p_reader
           AND NOT EXISTS (
               SELECT 1 FROM thread_metadata r
                WHERE r.community_id = p_cid AND r.event_id = e.id AND r.depth > 0
                  AND (NOT r.broadcast OR chat_is_thread_marker(e.tags, r.root_event_id)))
           AND NOT (v_dm AND EXISTS (
               SELECT 1 FROM user_block_periods bp
                WHERE bp.community_id = p_cid AND bp.blocker = p_reader
                  AND bp.blocked = e.pubkey AND bp.started_at <= e.received_at
                  AND (bp.ended_at IS NULL OR e.received_at < bp.ended_at)))
           AND EXISTS (
               SELECT 1 FROM event_mentions em
                WHERE em.community_id = p_cid AND em.pubkey_hex = v_reader_hex
                  AND em.event_id = e.id)
         LIMIT 100) n;
END
$$;

CREATE OR REPLACE FUNCTION chat_thread_unread(
    p_cid UUID, p_chid UUID, p_root BYTEA, p_reader BYTEA, p_after BIGINT,
    OUT unread INTEGER, OUT mentions INTEGER)
LANGUAGE plpgsql STABLE AS $$
DECLARE
    v_dm BOOLEAN;
    v_reader_hex TEXT := encode(p_reader, 'hex');
BEGIN
    v_dm := dm_is_one_to_one(p_cid, p_chid);
    SELECT count(*) INTO unread FROM (
        SELECT 1 FROM thread_metadata r
          JOIN events e ON e.community_id = r.community_id
           AND e.created_at = r.event_created_at AND e.id = r.event_id
         WHERE r.community_id = p_cid AND r.root_event_id = p_root AND r.depth > 0
           AND e.channel_sequence > COALESCE(p_after, 0)
           AND e.kind IN (9, 40002, 45001, 45003) AND e.deleted_at IS NULL
           AND e.pubkey <> p_reader
           AND NOT chat_is_thread_marker(e.tags, p_root)
           AND NOT (v_dm AND EXISTS (
               SELECT 1 FROM user_block_periods bp
                WHERE bp.community_id = p_cid AND bp.blocker = p_reader
                  AND bp.blocked = e.pubkey AND bp.started_at <= e.received_at
                  AND (bp.ended_at IS NULL OR e.received_at < bp.ended_at)))
         LIMIT 100) n;
    SELECT count(*) INTO mentions FROM (
        SELECT 1 FROM thread_metadata r
          JOIN events e ON e.community_id = r.community_id
           AND e.created_at = r.event_created_at AND e.id = r.event_id
         WHERE r.community_id = p_cid AND r.root_event_id = p_root AND r.depth > 0
           AND e.channel_sequence > COALESCE(p_after, 0)
           AND e.kind IN (9, 40002, 45001, 45003) AND e.deleted_at IS NULL
           AND e.pubkey <> p_reader
           AND NOT chat_is_thread_marker(e.tags, p_root)
           AND NOT (v_dm AND EXISTS (
               SELECT 1 FROM user_block_periods bp
                WHERE bp.community_id = p_cid AND bp.blocker = p_reader
                  AND bp.blocked = e.pubkey AND bp.started_at <= e.received_at
                  AND (bp.ended_at IS NULL OR e.received_at < bp.ended_at)))
           AND EXISTS (
               SELECT 1 FROM event_mentions em
                WHERE em.community_id = p_cid AND em.pubkey_hex = v_reader_hex
                  AND em.event_id = e.id)
         LIMIT 100) n;
END
$$;

-- Stored counts of current blockers were made with the old rule (every
-- message of a blocked person hidden, in group DMs too). Refill the DMs the
-- blocker shares with the blocked person.
DO $$
DECLARE
    room RECORD;
BEGIN
    FOR room IN
        SELECT DISTINCT c.community_id AS cid, c.id AS chid
          FROM user_blocks ub
          JOIN communities co ON co.id = ub.community_id AND co.deletion_state = 'active'
          JOIN channel_members a ON a.community_id = ub.community_id AND a.pubkey = ub.blocker
          JOIN channel_members o ON o.community_id = a.community_id
           AND o.channel_id = a.channel_id AND o.pubkey = ub.blocked
          JOIN channels c ON c.community_id = a.community_id AND c.id = a.channel_id
         WHERE c.channel_type = 'dm' AND c.deleted_at IS NULL
         ORDER BY 1, 2
    LOOP
        PERFORM refill_chat_stored_state_channel(room.cid, room.chid);
    END LOOP;
END
$$;
