-- Close and reopen marker replies are no longer counted as thread replies.
-- Remove markers stored before that change from their parent's reply_count
-- and their root's descendant_count, so the counters match what new writes
-- and deletes keep. Deleted markers were already decremented when they were
-- deleted. A marker is a reply with a kuzz:thread-closed or
-- kuzz:thread-reopened tag whose value is not empty (the desktop's
-- isThreadMarkerReply). Markers are few; the GIN index on events.tags finds
-- them.
CREATE TEMP TABLE marker_replies ON COMMIT DROP AS
SELECT tm.community_id, tm.parent_event_id, tm.root_event_id
FROM events e
JOIN thread_metadata tm
  ON tm.community_id = e.community_id AND tm.event_id = e.id
-- Communities being deleted are write-fenced; their rows are going away.
JOIN communities c
  ON c.id = tm.community_id AND c.deletion_state = 'active'
WHERE (e.tags @> '[["kuzz:thread-closed"]]' OR e.tags @> '[["kuzz:thread-reopened"]]')
  AND e.deleted_at IS NULL
  AND tm.parent_event_id IS NOT NULL
  AND EXISTS (
      SELECT 1
      FROM jsonb_array_elements(e.tags) AS marker_tag(tag)
      WHERE marker_tag.tag->>0 IN ('kuzz:thread-closed', 'kuzz:thread-reopened')
        AND COALESCE(marker_tag.tag->>1, '') <> ''
  );

UPDATE thread_metadata tm
SET reply_count = GREATEST(tm.reply_count - m.n, 0)
FROM (
    SELECT community_id, parent_event_id AS event_id, count(*)::int AS n
    FROM marker_replies
    GROUP BY community_id, parent_event_id
) m
WHERE tm.community_id = m.community_id AND tm.event_id = m.event_id;

UPDATE thread_metadata tm
SET descendant_count = GREATEST(tm.descendant_count - m.n, 0)
FROM (
    SELECT community_id, root_event_id AS event_id, count(*)::int AS n
    FROM marker_replies
    WHERE root_event_id IS NOT NULL
    GROUP BY community_id, root_event_id
) m
WHERE tm.community_id = m.community_id AND tm.event_id = m.event_id;
