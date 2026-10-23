-- Redefines 0089. An agent's owner reading the agent's DM or group chat
-- without being in it (read-only owner view) sees what the agent sees: the
-- owner's floor is the earliest floor of their agents in the room. Other
-- non-members of a cutting room still get the largest sequence.
CREATE OR REPLACE FUNCTION channel_history_floor(p_cid UUID, p_chid UUID, p_reader BYTEA)
RETURNS BIGINT
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT CASE WHEN NOT channel_cuts_history(p_cid, p_chid) THEN 0
        ELSE COALESCE((
            SELECT CASE WHEN hm.join_cutoff_is_legacy THEN 0 ELSE hm.join_sequence END
              FROM channel_members hm
             WHERE hm.community_id = p_cid AND hm.channel_id = p_chid
               AND hm.pubkey = p_reader AND hm.removed_at IS NULL),
            (SELECT MIN(CASE WHEN am.join_cutoff_is_legacy THEN 0 ELSE am.join_sequence END)
               FROM channel_members am
               JOIN users au ON au.community_id = am.community_id AND au.pubkey = am.pubkey
               JOIN channels ac ON ac.community_id = am.community_id AND ac.id = am.channel_id
              WHERE am.community_id = p_cid AND am.channel_id = p_chid
                AND am.removed_at IS NULL
                AND au.agent_owner_pubkey = p_reader
                AND (ac.channel_type = 'dm' OR ac.group_chat)),
            9223372036854775807)
    END
$$;
