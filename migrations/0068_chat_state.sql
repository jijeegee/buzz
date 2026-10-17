-- ── Server-side chat state: read positions, thread follows, thread state ─────
-- Stage S1 of the Chats server design. Additive only: new nullable or
-- constant-default columns (no table rewrite) and two new tenant tables. The
-- relay writes them only while KUZZ_CHAT_STATE_WRITES is on; with the switch
-- off nothing reads or writes them.
--
-- Privacy: read positions and follows are stored in plain form so every device
-- of a user sees the same list. They are deleted with the community (purge
-- list) and must be kept out of logs.

-- Room read position, in the channel's `events.channel_sequence` space.
-- `read_origin`: 'backfill' (set by the one-off backfill, may still be lowered
-- once by a client upload) or 'user' (set by an explicit client read).
ALTER TABLE channel_members ADD COLUMN read_seq BIGINT;
ALTER TABLE channel_members ADD COLUMN read_origin TEXT
    CHECK (read_origin IN ('backfill', 'user'));
ALTER TABLE channel_members ADD COLUMN unread_mark BOOLEAN NOT NULL DEFAULT FALSE;

-- Latest main-timeline message (top-level messages and broadcast replies).
ALTER TABLE channels ADD COLUMN main_last_seq BIGINT;
ALTER TABLE channels ADD COLUMN main_last_at TIMESTAMPTZ;
ALTER TABLE channels ADD COLUMN main_last_event_id BYTEA;

-- One row per (thread, channel member) who follows, auto-follows, or has
-- unfollowed a thread. Rows exist only for channel members.
CREATE TABLE thread_follows (
    community_id   UUID NOT NULL REFERENCES communities(id),
    root_event_id  BYTEA NOT NULL CHECK (length(root_event_id) = 32),
    pubkey         BYTEA NOT NULL CHECK (length(pubkey) = 32),
    channel_id     UUID NOT NULL,
    follow_state   TEXT NOT NULL CHECK (follow_state IN ('auto', 'followed', 'unfollowed')),
    follow_origin  TEXT NOT NULL CHECK (follow_origin IN ('backfill', 'live', 'user')),
    read_seq       BIGINT,
    unread_mark    BOOLEAN NOT NULL DEFAULT FALSE,
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (community_id, root_event_id, pubkey),
    FOREIGN KEY (community_id, channel_id)
        REFERENCES channels (community_id, id) ON DELETE CASCADE
);

CREATE INDEX idx_thread_follows_pubkey ON thread_follows (community_id, pubkey);

-- Per-thread activity and close state. Separate from thread_metadata because
-- the root's depth-0 metadata row exists only after the first reply.
-- `last_seq`/`last_at` ignore close and reopen marker replies.
CREATE TABLE thread_state (
    community_id   UUID NOT NULL REFERENCES communities(id),
    root_event_id  BYTEA NOT NULL CHECK (length(root_event_id) = 32),
    channel_id     UUID NOT NULL,
    last_seq       BIGINT,
    last_at        TIMESTAMPTZ,
    closed_at      TIMESTAMPTZ,
    closed_by      BYTEA,
    PRIMARY KEY (community_id, root_event_id),
    FOREIGN KEY (community_id, channel_id)
        REFERENCES channels (community_id, id) ON DELETE CASCADE
);

SELECT attach_community_write_fence('thread_follows');
SELECT attach_community_write_fence('thread_state');

-- Unread counts are computed on read from the channel sequence. Built without
-- CONCURRENTLY, matching migration 0065 (sqlx runs each migration inside a
-- transaction). On a large database an operator may pre-build it by hand per
-- partition with CREATE INDEX CONCURRENTLY and attach it; IF NOT EXISTS then
-- makes this statement a no-op.
CREATE INDEX IF NOT EXISTS idx_events_community_channel_sequence
    ON events (community_id, channel_id, channel_sequence);
