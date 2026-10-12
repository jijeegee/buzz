-- ── Inbox summary: newest replied-to thread roots per room ──────────────────
-- The /query `inbox_summary` envelope reads, for each room the reader belongs
-- to, the most recently replied-to thread roots. Without this index every
-- root in each room is read and sorted; with it each room reads at most one
-- page of roots in last_reply_at order.
--
-- Partial on depth = 0 AND reply_count > 0: only roots that have replies are
-- inbox rows, which keeps the index small.
--
-- Lock note: built without CONCURRENTLY, matching migrations 0004 and 0027 —
-- sqlx runs each migration inside a transaction. On a large database an
-- operator may pre-build it by hand with CREATE INDEX CONCURRENTLY; IF NOT
-- EXISTS then makes this migration a no-op.

CREATE INDEX IF NOT EXISTS idx_thread_metadata_inbox
    ON thread_metadata (community_id, channel_id, last_reply_at DESC, event_id DESC)
    WHERE depth = 0 AND reply_count > 0;
