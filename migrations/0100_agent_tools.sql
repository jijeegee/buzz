-- Owner tools on/off per agent key (AI tools plan §3-4, stage S1), and the
-- relay's log of what agents did for their owners (§8). The ALTER needs an
-- ACCESS EXCLUSIVE lock on key_registry; fail quickly rather than queue.
SET LOCAL lock_timeout = '1s';

-- `tool_level` is the owner's switch: NULL (never set, acts as limited),
-- `full` or `limited`. It only counts in `tool_community_id`, the community
-- whose host the owner set it through (not `community_id`: key_registry is
-- operator-global, and the deletion catalog must not see it as scoped).
-- `respond_class` is who the agent answers, kept so `full` + `anyone` is
-- never stored. Nullable, no defaults, no FK, checks NOT VALID: a catalog
-- change only, no table rewrite or scan.
ALTER TABLE key_registry
    ADD COLUMN tool_level TEXT,
    ADD COLUMN tool_community_id UUID,
    ADD COLUMN respond_class TEXT,
    ADD COLUMN tool_set_at TIMESTAMPTZ;
ALTER TABLE key_registry
    ADD CONSTRAINT key_registry_tool_level_check
        CHECK (tool_level IN ('full', 'limited')) NOT VALID,
    ADD CONSTRAINT key_registry_respond_class_check
        CHECK (respond_class IN ('owner-only', 'allowlist', 'anyone')) NOT VALID,
    ADD CONSTRAINT key_registry_tool_shape
        CHECK (tool_level IS NULL OR tool_community_id IS NOT NULL) NOT VALID,
    ADD CONSTRAINT key_registry_full_never_anyone
        CHECK (NOT (tool_level = 'full' AND respond_class = 'anyone')) NOT VALID;

-- One row per thing an agent did for its owner (or tried to): who, what, on
-- which computer, how it ended. No payload. Community-scoped with its write
-- fence, so a community purge takes it; kept 90 days.
CREATE TABLE agent_actions (
    community_id UUID NOT NULL,
    id           UUID NOT NULL DEFAULT gen_random_uuid(),
    owner_pubkey BYTEA NOT NULL CHECK (length(owner_pubkey) = 32),
    agent_pubkey BYTEA NOT NULL CHECK (length(agent_pubkey) = 32),
    action       TEXT NOT NULL CHECK (char_length(action) BETWEEN 1 AND 64),
    target       TEXT CHECK (char_length(target) <= 256),
    computer     TEXT CHECK (char_length(computer) <= 128),
    status       TEXT NOT NULL CHECK (status IN ('ok', 'refused', 'failed')),
    source       TEXT NOT NULL CHECK (source IN ('relay', 'app')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, id)
);

CREATE INDEX agent_actions_owner_time ON agent_actions (community_id, owner_pubkey, created_at);
CREATE INDEX agent_actions_time ON agent_actions (created_at);

SELECT attach_community_write_fence('agent_actions');
