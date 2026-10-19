-- Enforce channels.history_policy = 'since_join' on reads (0060 stored it,
-- nothing read it). One rule shared by every read path.

-- since_join history floor: the reader's current join_sequence in a
-- members-only (not `open`) room whose policy is `since_join`, else 0. Rows
-- with channel_sequence <= floor are hidden from that reader. Legacy-stamped
-- memberships keep all history. Channel access itself is checked elsewhere.
CREATE OR REPLACE FUNCTION channel_history_floor(p_cid UUID, p_chid UUID, p_reader BYTEA)
RETURNS BIGINT
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT COALESCE((
        SELECT hm.join_sequence FROM channels hc
        JOIN channel_members hm
          ON hm.community_id = hc.community_id AND hm.channel_id = hc.id
        WHERE hc.community_id = p_cid AND hc.id = p_chid
          AND hc.history_policy = 'since_join' AND hc.visibility <> 'open'
          AND hm.pubkey = p_reader AND hm.removed_at IS NULL
          AND NOT hm.join_cutoff_is_legacy), 0)
$$;

-- True when `reader` must not see the channel event with order number `seq`.
CREATE OR REPLACE FUNCTION channel_event_before_join(p_cid UUID, p_chid UUID, p_seq BIGINT, p_reader BYTEA)
RETURNS BOOLEAN
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT p_chid IS NOT NULL AND p_seq <= channel_history_floor(p_cid, p_chid, p_reader)
$$;
