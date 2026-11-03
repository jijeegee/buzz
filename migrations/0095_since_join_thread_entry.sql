-- since_join, third step (0088, 0089 first): a member who joined after a
-- thread's root and is @mentioned in a reply enters that thread from the
-- mention. They see the mention and every later reply of that thread, never
-- the root or the replies before it. Members who joined before the root are
-- untouched (they see the whole thread).
--
-- The entry lives on the reader's thread_follows row. It is access, not a
-- setting: only the reply ingest writes it, follow/read changes never clear
-- it, and a rejoin of the room invalidates it (an entry counts only above the
-- reader's current join floor).

ALTER TABLE thread_follows
    ADD COLUMN entry_seq BIGINT,
    ADD COLUMN entry_root_seq BIGINT;

CREATE INDEX idx_thread_follows_entry ON thread_follows
    (community_id, channel_id, pubkey, entry_root_seq) WHERE entry_seq IS NOT NULL;

-- The reader's entry into the thread whose root has channel sequence
-- `p_root_seq`, when it is valid above `p_floor` (their join floor); NULL
-- otherwise.
CREATE OR REPLACE FUNCTION channel_thread_entry_seq(
    p_cid UUID, p_chid UUID, p_root_seq BIGINT, p_reader BYTEA, p_floor BIGINT)
RETURNS BIGINT
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT tf.entry_seq FROM thread_follows tf
     WHERE tf.community_id = p_cid AND tf.channel_id = p_chid AND tf.pubkey = p_reader
       AND tf.entry_root_seq = p_root_seq AND tf.entry_seq IS NOT NULL
       AND tf.entry_seq > p_floor
     LIMIT 1
$$;

-- True when `reader`, whose join floor in the room is `p_floor` (0: nothing
-- cut), must not see the event with sequence `p_seq` and thread root sequence
-- `p_root_seq` (0 for anything but a reply). A reply in a thread from before
-- the join is visible from the reader's entry on.
CREATE OR REPLACE FUNCTION channel_event_hidden_at(
    p_cid UUID, p_chid UUID, p_seq BIGINT, p_root_seq BIGINT, p_reader BYTEA, p_floor BIGINT)
RETURNS BOOLEAN
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT p_chid IS NOT NULL AND p_floor > 0 AND (
        p_seq <= p_floor
        OR (p_root_seq > 0 AND p_root_seq < p_seq AND p_root_seq <= p_floor
            AND COALESCE(p_seq < channel_thread_entry_seq(p_cid, p_chid, p_root_seq, p_reader,
                                                          p_floor), true)))
$$;

-- channel_event_hidden_at with the reader's current floor.
CREATE OR REPLACE FUNCTION channel_event_hidden(
    p_cid UUID, p_chid UUID, p_seq BIGINT, p_root_seq BIGINT, p_reader BYTEA)
RETURNS BOOLEAN
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT p_chid IS NOT NULL
       AND channel_event_hidden_at(p_cid, p_chid, p_seq, p_root_seq, p_reader,
                                   channel_history_floor(p_cid, p_chid, p_reader))
$$;

-- True when `reader` may open the thread whose root has sequence
-- `p_root_seq`: the root is visible, or they entered the thread.
CREATE OR REPLACE FUNCTION channel_thread_open_to(
    p_cid UUID, p_chid UUID, p_root_seq BIGINT, p_reader BYTEA)
RETURNS BOOLEAN
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT CASE WHEN p_chid IS NULL THEN true ELSE (
        SELECT f.v = 0 OR p_root_seq > f.v
            OR channel_thread_entry_seq(p_cid, p_chid, p_root_seq, p_reader, f.v) IS NOT NULL
          FROM (SELECT channel_history_floor(p_cid, p_chid, p_reader) AS v) f) END
$$;

-- Redefines 0089: targets are judged with the entry rule.
CREATE OR REPLACE FUNCTION channel_event_targets_before_join(
    p_cid UUID, p_chid UUID, p_kind INTEGER, p_tags JSONB, p_reader BYTEA)
RETURNS BOOLEAN
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT CASE
        WHEN p_kind <> ALL (ARRAY[5, 7, 9005, 40003, 39005]) THEN false
        WHEN p_chid IS NOT NULL AND channel_history_floor(p_cid, p_chid, p_reader) = 0 THEN false
        -- A deletion is hidden only when everything it deletes is: it also
        -- removes visible messages from the reader's view.
        WHEN p_kind IN (5, 9005) THEN COALESCE((
            SELECT bool_and(channel_event_hidden(te.community_id, te.channel_id,
                    te.channel_sequence, te.thread_root_sequence, p_reader))
              FROM jsonb_array_elements(p_tags) t
              JOIN events te ON te.community_id = p_cid
               AND te.id = CASE WHEN t->>1 ~ '^[0-9a-fA-F]{64}$' THEN decode(t->>1, 'hex') END
             WHERE t->>0 = 'e'), false)
        ELSE EXISTS (
            SELECT 1 FROM jsonb_array_elements(p_tags) t
            JOIN events te ON te.community_id = p_cid
             AND te.id = CASE WHEN t->>1 ~ '^[0-9a-fA-F]{64}$' THEN decode(t->>1, 'hex') END
            WHERE t->>0 = 'e'
              AND channel_event_hidden(te.community_id, te.channel_id,
                    te.channel_sequence, te.thread_root_sequence, p_reader))
    END
$$;

-- Redefines 0089: "also send to channel" copies of replies in an entered
-- thread count from the entry.
CREATE OR REPLACE FUNCTION chat_room_unread(
    p_cid UUID, p_chid UUID, p_reader BYTEA, p_after BIGINT,
    OUT unread INTEGER, OUT mentions INTEGER)
LANGUAGE plpgsql STABLE AS $$
DECLARE
    v_dm BOOLEAN;
    v_reader_hex TEXT := encode(p_reader, 'hex');
    v_after BIGINT := COALESCE(p_after, 0);
    v_floor BIGINT;
BEGIN
    v_dm := dm_is_one_to_one(p_cid, p_chid);
    -- (The fail-closed floor of a missing membership is ignored: the tally
    -- trigger counts a row before it is in the table and passes its own cut.)
    v_floor := COALESCE(NULLIF(channel_history_floor(p_cid, p_chid, p_reader),
                               9223372036854775807), 0);
    v_after := GREATEST(v_after, v_floor);
    SELECT count(*) INTO unread FROM (
        SELECT 1 FROM events e
         WHERE e.community_id = p_cid AND e.channel_id = p_chid
           AND e.channel_sequence > v_after
           AND NOT channel_event_hidden_at(p_cid, p_chid, e.channel_sequence,
                                           e.thread_root_sequence, p_reader, v_floor)
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
           AND e.channel_sequence > v_after
           AND NOT channel_event_hidden_at(p_cid, p_chid, e.channel_sequence,
                                           e.thread_root_sequence, p_reader, v_floor)
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
