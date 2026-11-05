-- Goals as rows (one per goal) instead of one document per conversation, so a
-- room has no size cap and a write touches only what it changes. A finished
-- small goal is deleted outright; signed kind 40111 events keep the change log.
CREATE TABLE goals (
    community_id  UUID NOT NULL REFERENCES communities(id),
    channel_id    UUID NOT NULL,
    id            TEXT NOT NULL,
    parent_id     TEXT,
    title         TEXT NOT NULL,
    note          TEXT NOT NULL DEFAULT '',
    status        TEXT NOT NULL DEFAULT 'open'
        CHECK (status IN ('open', 'in_progress', 'done', 'dropped')),
    order_key     BIGINT NOT NULL DEFAULT 0,
    assignees     TEXT[] NOT NULL DEFAULT '{}',
    updated_by    TEXT,
    updated_at    TIMESTAMPTZ NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (community_id, channel_id, id),
    FOREIGN KEY (community_id, channel_id)
        REFERENCES channels (community_id, id) ON DELETE CASCADE,
    -- NO ACTION, not CASCADE: which branches survive is the deletion rule's
    -- decision. Checked at statement end, so one DELETE may take a whole branch.
    FOREIGN KEY (community_id, channel_id, parent_id)
        REFERENCES goals (community_id, channel_id, id)
);

CREATE INDEX idx_goals_parent ON goals (community_id, channel_id, parent_id);
-- One layer 1 goal per conversation.
CREATE UNIQUE INDEX idx_goals_one_root ON goals (community_id, channel_id)
    WHERE parent_id IS NULL;

-- A thread root or single message linked to a goal; each links to one goal.
CREATE TABLE goal_links (
    community_id  UUID NOT NULL,
    channel_id    UUID NOT NULL,
    event_id      BYTEA NOT NULL CHECK (length(event_id) = 32),
    goal_id       TEXT NOT NULL,
    PRIMARY KEY (community_id, channel_id, event_id),
    FOREIGN KEY (community_id, channel_id, goal_id)
        REFERENCES goals (community_id, channel_id, id) ON DELETE CASCADE
);

CREATE INDEX idx_goal_links_goal ON goal_links (community_id, channel_id, goal_id);

SELECT attach_community_write_fence('goals');
SELECT attach_community_write_fence('goal_links');
