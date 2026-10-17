-- ── Chats: where a thread read position came from ─────────────────────────────
-- Stage S2b of the Chats server design. A new app's first run uploads the
-- read positions and follows it kept on the device (`chat_import`). The
-- server may change a read position from that upload only while it still
-- holds the backfilled value, the same rule `channel_members.read_origin`
-- gives room read positions:
--   'backfill' set by the one-off backfill; one upload may raise or lower it.
--   'user'     set by the reader (a read, an upload, or their own reply).
--   NULL       set by the server for someone else's activity (a mention).
-- Additive and nullable: no table rewrite.
ALTER TABLE thread_follows ADD COLUMN read_origin TEXT
    CHECK (read_origin IN ('backfill', 'user'));

-- Rows the backfill wrote before this column existed hold backfilled read
-- positions.
UPDATE thread_follows SET read_origin = 'backfill'
WHERE follow_origin = 'backfill' AND read_origin IS NULL;
