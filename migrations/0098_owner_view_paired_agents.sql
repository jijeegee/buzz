-- Read-only owner view for agents paired with `kuzz pair`. Their owner is not
-- in `users.agent_owner_pubkey`, only in `key_registry`.

-- The agents paired to the person with account key `p_owner`, disconnected ones
-- included. Most readers have none, so this is one lookup per query, not one
-- per room member, and it doesn't need a `users` row: pairing writes none.
CREATE OR REPLACE FUNCTION paired_agents_of(p_owner BYTEA)
RETURNS SETOF BYTEA
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT r.pubkey
      FROM account_key_backups b
      JOIN key_registry r ON r.principal_id = b.account_id
     WHERE b.pubkey = p_owner AND r.role = 'external' AND r.revoked_at IS NULL
$$;

-- Redefines 0091: the owner's floor also counts their paired agents.
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
               LEFT JOIN users au ON au.community_id = am.community_id AND au.pubkey = am.pubkey
               JOIN channels ac ON ac.community_id = am.community_id AND ac.id = am.channel_id
              WHERE am.community_id = p_cid AND am.channel_id = p_chid
                AND am.removed_at IS NULL
                AND (au.agent_owner_pubkey = p_reader
                     OR am.pubkey IN (SELECT paired_agents_of(p_reader)))
                AND (ac.channel_type = 'dm' OR ac.group_chat)),
            9223372036854775807)
    END
$$;
