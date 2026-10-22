-- Groups: public addresses, invite links, "who can add me" and invitations.
--
-- Additive only: the previous relay image ignores every column and table here.

-- Public address of an open group (`/api/groups/by-handle/{handle}`). Stored
-- lowercase; cleared when the group becomes private. Unique among live groups
-- of one community.
ALTER TABLE channels ADD COLUMN handle TEXT
    CONSTRAINT chk_channels_handle CHECK (handle ~ '^[a-z0-9_]{3,32}$');
CREATE UNIQUE INDEX idx_channels_handle ON channels (community_id, lower(handle))
    WHERE handle IS NOT NULL AND deleted_at IS NULL;

-- Who may share the invite link and add people: every member or only
-- owners/admins.
ALTER TABLE channels ADD COLUMN invite_policy TEXT NOT NULL DEFAULT 'members'
    CONSTRAINT chk_channels_invite_policy CHECK (invite_policy IN ('members', 'admins'));

-- One shareable link per group (`/g/<code>`). Resetting revokes the active row
-- and inserts a new one; revoked rows stay for audit.
CREATE TABLE group_invite_links (
    community_id UUID NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    channel_id   UUID NOT NULL,
    code         TEXT NOT NULL CHECK (code ~ '^[A-Za-z0-9_-]{22,64}$'),
    created_by   BYTEA NOT NULL CHECK (length(created_by) = 32),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at   TIMESTAMPTZ,
    PRIMARY KEY (community_id, code),
    FOREIGN KEY (community_id, channel_id)
        REFERENCES channels (community_id, id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX idx_group_invite_links_active
    ON group_invite_links (community_id, channel_id) WHERE revoked_at IS NULL;

-- "Who can add me to groups". No row means 'friends'.
CREATE TABLE group_add_settings (
    community_id UUID NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    pubkey       BYTEA NOT NULL CHECK (length(pubkey) = 32),
    policy       TEXT NOT NULL DEFAULT 'friends' CHECK (policy IN ('friends', 'everyone')),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, pubkey)
);

-- An add that the target's setting refused becomes an invitation. Pending
-- rows older than 30 days count as expired when read.
CREATE TABLE group_invitations (
    community_id UUID NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    id           UUID NOT NULL DEFAULT gen_random_uuid(),
    channel_id   UUID NOT NULL,
    inviter      BYTEA NOT NULL CHECK (length(inviter) = 32),
    invitee      BYTEA NOT NULL CHECK (length(invitee) = 32),
    status       TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'accepted', 'declined', 'expired')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    responded_at TIMESTAMPTZ,
    PRIMARY KEY (community_id, id),
    FOREIGN KEY (community_id, channel_id)
        REFERENCES channels (community_id, id) ON DELETE CASCADE,
    CONSTRAINT group_invitations_not_self CHECK (inviter <> invitee)
);
CREATE UNIQUE INDEX idx_group_invitations_pending
    ON group_invitations (community_id, channel_id, invitee) WHERE status = 'pending';
CREATE INDEX idx_group_invitations_invitee
    ON group_invitations (community_id, invitee) WHERE status = 'pending';
-- Per-inviter daily invitation cap.
CREATE INDEX idx_group_invitations_inviter
    ON group_invitations (community_id, inviter, created_at);

SELECT attach_community_write_fence('group_add_settings');
SELECT attach_community_write_fence('group_invitations');
SELECT attach_community_write_fence('group_invite_links');
