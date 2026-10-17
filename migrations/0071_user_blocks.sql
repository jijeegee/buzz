-- Silent blocks (Kuzz friends/block/report). A row means `blocker` no longer
-- receives `blocked`'s DM events: live fan-out, history, unread counts and
-- push all skip them, while `blocked` keeps seeing normal acceptance. Rows are
-- private to the blocker; nothing tells `blocked` the row exists.
--
-- Pubkeys are the 32-byte messaging keys events are signed with (the account
-- key for key-backup accounts, the principal id for token accounts).
CREATE TABLE user_blocks (
    community_id UUID NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    blocker      BYTEA NOT NULL CHECK (length(blocker) = 32),
    blocked      BYTEA NOT NULL CHECK (length(blocked) = 32),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, blocker, blocked),
    CONSTRAINT user_blocks_not_self CHECK (blocker <> blocked)
);

-- Delivery asks "who blocked this author?"; the primary key serves the
-- blocker's own list.
CREATE INDEX idx_user_blocks_blocked ON user_blocks (community_id, blocked);

SELECT attach_community_write_fence('user_blocks');

-- "Let people find me by ID or email" (default on). Exact-match lookup only.
ALTER TABLE principals ADD COLUMN discoverable BOOLEAN NOT NULL DEFAULT true;

-- Exact, case-insensitive email lookup for adding a friend.
CREATE INDEX identities_email_lower ON identities (lower(email)) WHERE email IS NOT NULL;
