-- Enforce channels.history_policy = 'since_join' on reads (0060 stored it,
-- nothing read it). One predicate shared by every read path.

-- since_join history: true when `reader` must not see the channel event with
-- order number `seq`. Only members-only rooms (not `open`) whose policy is
-- `since_join` cut; legacy-stamped memberships keep all history. Reader access
-- to the channel itself is checked elsewhere; this only removes pre-join rows.
CREATE OR REPLACE FUNCTION channel_event_before_join(p_cid UUID, p_chid UUID, p_seq BIGINT, p_reader BYTEA)
RETURNS BOOLEAN
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT p_chid IS NOT NULL AND EXISTS (
        SELECT 1 FROM channels hc
        JOIN channel_members hm
          ON hm.community_id = hc.community_id AND hm.channel_id = hc.id
        WHERE hc.community_id = p_cid AND hc.id = p_chid
          AND hc.history_policy = 'since_join' AND hc.visibility <> 'open'
          AND hm.pubkey = p_reader AND hm.removed_at IS NULL
          AND NOT hm.join_cutoff_is_legacy
          AND p_seq <= hm.join_sequence)
$$;
