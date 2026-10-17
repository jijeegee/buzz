-- ── Chats: where a thread read position came from ─────────────────────────────
-- Stage S2b of the Chats server design. A new app's first run uploads the
-- read positions and follows it kept on the device (`chat_import`). The
-- server may change a read position from that upload only while it still
-- holds the backfilled value, the same rule `channel_members.read_origin`
-- gives room read positions:
--   'backfill' set by the one-off backfill; one upload may raise or lower it.
--   'user'     set by the reader (a read, an upload, or their own reply).
--   NULL       set by the server for someone else's activity (a mention).
--
-- Deploys together with 0067, before the chat-state switch is turned on and
-- the backfill runs, so thread_follows is still empty here: no existing rows
-- to mark, and the inline CHECK (the convention of every migration in this
-- repo) validates nothing. A row with NULL read_origin and follow_origin
-- 'backfill' is still read as backfill by `chat_import`.
ALTER TABLE thread_follows ADD COLUMN read_origin TEXT
    CHECK (read_origin IN ('backfill', 'user'));
