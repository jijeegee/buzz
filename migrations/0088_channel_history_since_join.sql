-- Enforce channels.history_policy = 'since_join' on reads (0060 stored it,
-- nothing read it). One rule shared by every read path.

-- since_join history floor: the reader's current join_sequence in a
-- members-only (not `open`) room whose policy is `since_join`, else 0. Rows
-- with channel_sequence <= floor are hidden from that reader. Legacy-stamped
-- memberships keep all history. Channel access itself is checked elsewhere.
-- Room state kinds (channel_kind_is_room_state) are never cut.
CREATE OR REPLACE FUNCTION channel_history_floor(p_cid UUID, p_chid UUID, p_reader BYTEA)
RETURNS BIGINT
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    -- No current membership in such a room (a replica that has not seen the
    -- join yet): fail closed with the largest sequence, hiding everything.
    SELECT CASE WHEN NOT EXISTS (
            SELECT 1 FROM channels hc
             WHERE hc.community_id = p_cid AND hc.id = p_chid
               AND hc.history_policy = 'since_join' AND hc.visibility <> 'open') THEN 0
        ELSE COALESCE((
            SELECT CASE WHEN hm.join_cutoff_is_legacy THEN 0 ELSE hm.join_sequence END
              FROM channel_members hm
             WHERE hm.community_id = p_cid AND hm.channel_id = p_chid
               AND hm.pubkey = p_reader AND hm.removed_at IS NULL),
            9223372036854775807)
    END
$$;

-- True when `reader` must not see the channel event with order number `seq`.
CREATE OR REPLACE FUNCTION channel_event_before_join(p_cid UUID, p_chid UUID, p_seq BIGINT, p_reader BYTEA)
RETURNS BOOLEAN
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT p_chid IS NOT NULL
       AND p_seq <= COALESCE(NULLIF(channel_history_floor(p_cid, p_chid, p_reader), 0), -1)
$$;

-- Current room state (replaceable/addressable kinds, room creation, canvas,
-- goal tree) stays visible to a new member: only the history is cut.
CREATE OR REPLACE FUNCTION channel_kind_is_room_state(p_kind INTEGER)
RETURNS BOOLEAN
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT p_kind IN (9007, 40100, 40110)
        OR p_kind BETWEEN 10000 AND 19999 OR p_kind BETWEEN 30000 AND 39999
$$;

-- True when the reaction, edit, deletion or thread summary (kind) with `tags`
-- names an event (`e` tag) that `reader` must not see. Other kinds: false.
CREATE OR REPLACE FUNCTION channel_event_targets_before_join(
    p_cid UUID, p_chid UUID, p_kind INTEGER, p_tags JSONB, p_reader BYTEA)
RETURNS BOOLEAN
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT CASE
        WHEN p_kind <> ALL (ARRAY[5, 7, 9005, 40003, 39005]) THEN false
        WHEN p_chid IS NOT NULL AND channel_history_floor(p_cid, p_chid, p_reader) = 0 THEN false
        ELSE EXISTS (
            SELECT 1 FROM jsonb_array_elements(p_tags) t
            JOIN events te ON te.community_id = p_cid
             AND te.id = CASE WHEN t->>1 ~ '^[0-9a-fA-F]{64}$' THEN decode(t->>1, 'hex') END
            WHERE t->>0 = 'e'
              AND channel_event_before_join(te.community_id, te.channel_id, te.channel_sequence,
                                            p_reader))
    END
$$;

-- Stored unread counts (0072/0073) skip messages from before the join.
CREATE OR REPLACE FUNCTION chat_room_unread(
    p_cid UUID, p_chid UUID, p_reader BYTEA, p_after BIGINT,
    OUT unread INTEGER, OUT mentions INTEGER)
LANGUAGE plpgsql STABLE AS $$
DECLARE
    v_dm BOOLEAN;
    v_reader_hex TEXT := encode(p_reader, 'hex');
    v_after BIGINT;
BEGIN
    v_dm := dm_is_one_to_one(p_cid, p_chid);
    -- since_join rooms: messages from before the reader's join never count.
    -- (The fail-closed floor of a missing membership is ignored: the tally
    -- trigger counts a row before it is in the table and passes its own cut.)
    v_after := GREATEST(COALESCE(p_after, 0),
                        NULLIF(channel_history_floor(p_cid, p_chid, p_reader), 9223372036854775807));
    SELECT count(*) INTO unread FROM (
        SELECT 1 FROM events e
         WHERE e.community_id = p_cid AND e.channel_id = p_chid
           AND e.channel_sequence > v_after
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

CREATE OR REPLACE FUNCTION tally_channel_member_unread() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
DECLARE
    v_after BIGINT := NEW.read_seq;
BEGIN
    IF NEW.removed_at IS NOT NULL THEN
        RETURN NEW;
    END IF;
    IF TG_OP = 'UPDATE' AND NEW.read_seq IS NOT DISTINCT FROM OLD.read_seq
       AND OLD.removed_at IS NULL THEN
        RETURN NEW;
    END IF;
    IF chat_read_to_end(NEW.community_id, NEW.channel_id, NEW.read_seq) THEN
        NEW.unread_count := 0;
        NEW.unread_mentions := 0;
    ELSE
        -- since_join rooms: count only after this (re)join. The table row is
        -- not this membership yet, so channel_history_floor cannot see it.
        IF NOT NEW.join_cutoff_is_legacy AND EXISTS (
            SELECT 1 FROM channels c
             WHERE c.community_id = NEW.community_id AND c.id = NEW.channel_id
               AND c.history_policy = 'since_join' AND c.visibility <> 'open') THEN
            v_after := GREATEST(COALESCE(v_after, 0), NEW.join_sequence);
        END IF;
        SELECT u.unread, u.mentions INTO NEW.unread_count, NEW.unread_mentions
          FROM chat_room_unread(NEW.community_id, NEW.channel_id, NEW.pubkey, v_after) u;
    END IF;
    IF TG_OP = 'UPDATE' AND OLD.removed_at IS NOT NULL THEN
        PERFORM chat_recount_thread_follows(
            NEW.community_id, NULL, NEW.channel_id, NEW.pubkey, NULL, NULL);
    END IF;
    RETURN NEW;
END
$$;

-- Recount members of since_join rooms whose stored counts predate this rule.
UPDATE channel_members cm
   SET (unread_count, unread_mentions) = (
       SELECT u.unread, u.mentions
         FROM chat_room_unread(cm.community_id, cm.channel_id, cm.pubkey, cm.read_seq) u)
  FROM channels c, communities co
 WHERE c.community_id = cm.community_id AND c.id = cm.channel_id
   AND c.history_policy = 'since_join' AND c.visibility <> 'open'
   AND co.id = cm.community_id AND co.deletion_state = 'active'
   AND cm.removed_at IS NULL AND NOT cm.join_cutoff_is_legacy;
