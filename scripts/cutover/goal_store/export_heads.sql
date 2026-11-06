-- Each room's goal tree head as one JSON line, for the goal_import example.
-- `threads` lists the linked ids that count as threads, the same test
-- `store/goals.rs` uses: a top-level message with replies.
WITH heads AS (
    SELECT DISTINCT ON (community_id, channel_id)
        community_id, channel_id, created_at, content
    FROM events
    WHERE kind = 40110 AND deleted_at IS NULL AND channel_id IS NOT NULL
    ORDER BY community_id, channel_id, created_at DESC, id ASC
),
links AS (
    SELECT h.community_id, h.channel_id, link
    FROM heads h,
        jsonb_array_elements(h.content::jsonb -> 'nodes') AS node,
        jsonb_array_elements_text(COALESCE(node -> 'threads', '[]')) AS link
)
SELECT json_build_object(
    'community', h.community_id,
    'channel', h.channel_id,
    'created_at', extract(epoch FROM h.created_at)::bigint,
    'content', h.content,
    'threads', COALESCE((
        SELECT json_agg(l.link)
        FROM links l
        JOIN thread_metadata t
            ON t.community_id = l.community_id AND t.channel_id = l.channel_id
            AND t.event_id = decode(l.link, 'hex')
        WHERE l.community_id = h.community_id AND l.channel_id = h.channel_id
            AND t.parent_event_id IS NULL AND t.reply_count > 0
    ), '[]'::json)
)
FROM heads h;
