-- Kuzz initial Postgres schema — multi-tenant.
--
-- Source of truth for fresh database setup. This is a clean, from-scratch
-- schema in which `community_id` is a first-class, server-resolved key on
-- every tenant-scoped row. It is NOT additive over the single-community
-- schema; the rewrite replaces it. Existing single-community deployments
-- migrate via the documented backfill migration (0002), which assigns all
-- pre-existing rows to one default community.
--
-- The governing contract is docs/multi-tenant-conformance.md. Every table
-- below cites the conformance surface it implements. The invariant behind the
-- whole schema (conformance "row zero"): a request's community is resolved
-- from the connection host by the server, never supplied by the client, and
-- every scoped row carries that immutable `community_id`.
--
-- Migration-lint obligations enforced by the Lane 0 lint harness:
--   1. Every tenant-scoped table has `community_id NOT NULL`.
--   2. No UNIQUE / PRIMARY KEY / FK on a scoped table is observable across
--      communities: each leads with `community_id` (or, for child rows whose
--      parent already pins the community, joins carry the community tuple).
--   3. `channels.community_id` is immutable (trigger below; no UPDATE path).
--   4. Operator-global tables are named in the explicit allowlist, not implied.

CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- ── Custom types ──────────────────────────────────────────────────────────────

CREATE TYPE channel_type AS ENUM ('stream', 'forum', 'dm', 'workflow');
CREATE TYPE channel_visibility AS ENUM ('open', 'private');
CREATE TYPE member_role AS ENUM ('owner', 'admin', 'member', 'guest', 'bot');
CREATE TYPE workflow_status AS ENUM ('active', 'disabled', 'archived');
CREATE TYPE run_status AS ENUM ('pending', 'running', 'waiting_approval', 'completed', 'failed', 'cancelled');
CREATE TYPE approval_status AS ENUM ('pending', 'granted', 'denied', 'expired');
CREATE TYPE delivery_method AS ENUM ('webhook', 'websocket');
CREATE TYPE subscription_status AS ENUM ('active', 'paused', 'deleted');
CREATE TYPE pause_reason AS ENUM ('user', 'system', 'rate_limit');
CREATE TYPE channel_add_policy AS ENUM ('anyone', 'owner_only', 'nobody');

-- ── Communities ───────────────────────────────────────────────────────────────
-- Conformance: row zero (host binding). The host map. `resolve_host(host)`
-- reads exactly one row here to mint the request's TenantContext. This table
-- is OPERATOR-GLOBAL: it is the registry of tenants, not itself tenant-scoped,
-- so it carries no `community_id` of its own (its `id` IS the community key).
-- Listed in the lint allowlist as operator-global.
--
-- Host normalization (Lane 0 contract): `host` is stored already-normalized —
-- ASCII-lowercased, trailing dot stripped, default port omitted. The UNIQUE is
-- on `lower(host)` belt-and-suspenders so `Relay.Example` and `relay.example`
-- can never become two tenants even if a writer forgets to normalize.
-- `resolve_host()` (kuzz-core) applies the identical normalization before
-- lookup, so resolution and storage agree by construction.

CREATE TABLE communities (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    host            VARCHAR(255) NOT NULL,
    signing_key     BYTEA,
    -- Per-community workspace icon (NIP-11 `icon`), set via kind:9033.
    -- Added by migration 0003; kept here so desired-state applies match.
    icon            TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    archived_at     TIMESTAMPTZ,
    deletion_state  TEXT NOT NULL DEFAULT 'active' CHECK (deletion_state IN ('active', 'quiescing', 'fenced', 'tombstone')),
    deletion_fence_generation BIGINT NOT NULL DEFAULT 0 CHECK (deletion_fence_generation >= 0),
    deleted_at      TIMESTAMPTZ,
    CONSTRAINT chk_communities_id_not_nil CHECK (id <> '00000000-0000-0000-0000-000000000000'::uuid)
);

CREATE UNIQUE INDEX idx_communities_host ON communities (lower(host));

-- ── Channels ──────────────────────────────────────────────────────────────────
-- Conformance: "Channels and channel membership". `community_id` immutable.
-- Channel UUIDs stay valid wire identifiers, but they are NOT globally unique:
-- the PK is `(community_id, id)`, so the same UUID may legitimately exist in two
-- communities (conformance lists "same channel UUID collision in two
-- communities" as a required isolation test). Handlers always carry `ctx`, so
-- `(ctx.community, h)` names exactly one channel; a client-supplied `h` can
-- never reach another community's channel.

CREATE TABLE channels (
    id              UUID NOT NULL DEFAULT gen_random_uuid(),
    community_id    UUID NOT NULL REFERENCES communities(id),
    name            VARCHAR(255) NOT NULL,
    channel_type    channel_type NOT NULL DEFAULT 'stream',
    visibility      channel_visibility NOT NULL DEFAULT 'open',
    description     TEXT,
    canvas          TEXT,
    created_by      BYTEA NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    archived_at     TIMESTAMPTZ,
    deleted_at      TIMESTAMPTZ,
    nip29_group_id  VARCHAR(255),
    topic_required  BOOLEAN NOT NULL DEFAULT FALSE,
    max_members     INT,
    topic           TEXT,
    topic_set_by    BYTEA,
    topic_set_at    TIMESTAMPTZ,
    purpose         TEXT,
    purpose_set_by  BYTEA,
    purpose_set_at  TIMESTAMPTZ,
    history_policy TEXT NOT NULL DEFAULT 'since_join' CHECK (history_policy IN ('all', 'since_join')),
    accepted_sequence BIGINT NOT NULL DEFAULT 0 CHECK (accepted_sequence >= 0),
    participant_hash BYTEA,
    ttl_seconds     INT,
    ttl_deadline    TIMESTAMPTZ,
    goals_enabled   BOOLEAN NOT NULL DEFAULT FALSE,
    -- Latest main-timeline message (migration 0068).
    main_last_seq   BIGINT,
    main_last_at    TIMESTAMPTZ,
    main_last_event_id BYTEA,
    -- Public address of an open group and who may invite (migration 0075).
    handle          TEXT CONSTRAINT chk_channels_handle CHECK (handle ~ '^[a-z0-9_]{3,32}$'),
    invite_policy   TEXT NOT NULL DEFAULT 'members'
        CONSTRAINT chk_channels_invite_policy CHECK (invite_policy IN ('members', 'admins')),
    -- Group photo URL on this relay's /media, or NULL (migration 0076).
    picture         TEXT,
    -- Ownerless group chat, set only at creation (migration 0079).
    group_chat      BOOLEAN NOT NULL DEFAULT false,
    PRIMARY KEY (community_id, id),
    CONSTRAINT chk_channels_id_not_nil CHECK (id <> '00000000-0000-0000-0000-000000000000'::uuid)
);

-- nip29 group id and DM participant hash are unique WITHIN a community, not globally.
CREATE UNIQUE INDEX idx_channels_nip29_group ON channels (community_id, nip29_group_id)
    WHERE nip29_group_id IS NOT NULL;
CREATE UNIQUE INDEX idx_channels_dm_hash ON channels (community_id, participant_hash)
    WHERE participant_hash IS NOT NULL;
CREATE UNIQUE INDEX idx_channels_handle ON channels (community_id, lower(handle))
    WHERE handle IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX idx_channels_community_type ON channels (community_id, channel_type);
CREATE INDEX idx_channels_community_visibility ON channels (community_id, visibility);
CREATE INDEX idx_channels_created_by ON channels (community_id, created_by);
CREATE INDEX idx_channels_ttl_expiry ON channels (ttl_deadline)
    WHERE ttl_seconds IS NOT NULL AND archived_at IS NULL AND deleted_at IS NULL;
-- Tenant-independent channel-id → community lookups (Db::communities_of_channels,
-- Db::community_of_channel) carry no community_id predicate, so no
-- community_id-leading index can serve them. Covering + partial: index-only scan.
-- Not UNIQUE — the same channel id may exist under more than one community.
CREATE INDEX idx_channels_id_live ON channels (id) INCLUDE (community_id)
    WHERE deleted_at IS NULL;

-- channels.community_id is immutable: a channel can never be re-tenanted.
-- (Conformance: "Migration lint forbids channel re-tenanting except through an
-- explicitly modeled admission path." We have no such path, so: hard block.)
CREATE FUNCTION channels_community_id_immutable() RETURNS TRIGGER AS $$
BEGIN
    IF NEW.community_id IS DISTINCT FROM OLD.community_id THEN
        RAISE EXCEPTION 'channels.community_id is immutable (channel % cannot be re-tenanted)', OLD.id
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_channels_community_id_immutable
    BEFORE UPDATE ON channels
    FOR EACH ROW EXECUTE FUNCTION channels_community_id_immutable();

-- ── Channel members ───────────────────────────────────────────────────────────
-- Conformance: "Channels and channel membership". PK leads with community_id.

CREATE TABLE channel_members (
    community_id UUID NOT NULL REFERENCES communities(id),
    channel_id  UUID NOT NULL,
    pubkey      BYTEA NOT NULL,
    role        member_role NOT NULL DEFAULT 'member',
    joined_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    join_sequence BIGINT NOT NULL DEFAULT 0 CHECK (join_sequence >= 0),
    join_cutoff_is_legacy BOOLEAN NOT NULL DEFAULT false,
    invited_by  BYTEA,
    removed_at  TIMESTAMPTZ,
    removed_by  BYTEA,
    hidden_at   TIMESTAMPTZ,
    -- Room read position in channel_sequence space (migration 0068).
    read_seq    BIGINT,
    read_origin TEXT CHECK (read_origin IN ('backfill', 'user')),
    unread_mark BOOLEAN NOT NULL DEFAULT FALSE,
    -- Stored unread counts, capped at 100 (migration 0072).
    unread_count    INTEGER NOT NULL DEFAULT 0,
    unread_mentions INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (community_id, channel_id, pubkey),
    FOREIGN KEY (community_id, channel_id)
        REFERENCES channels (community_id, id) ON DELETE CASCADE
);

CREATE INDEX idx_channel_members_pubkey ON channel_members (community_id, pubkey)
    WHERE removed_at IS NULL;

-- ── Users ─────────────────────────────────────────────────────────────────────
-- Conformance: "Users, profiles, NIP-05, and user search". One profile per
-- (community, pubkey): the same key reposts kind:0 in each community it joins.

CREATE TABLE users (
    community_id        UUID NOT NULL REFERENCES communities(id),
    pubkey              BYTEA NOT NULL,
    nip05_handle        VARCHAR(255),
    display_name        VARCHAR(255),
    avatar_url          TEXT,
    about               TEXT,
    agent_type          VARCHAR(255),
    capabilities        JSONB,
    okta_user_id        VARCHAR(255),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deactivated_at      TIMESTAMPTZ,
    metadata_event_id   BYTEA,
    agent_owner_pubkey  BYTEA,
    channel_add_policy  channel_add_policy NOT NULL DEFAULT 'anyone',
    PRIMARY KEY (community_id, pubkey),
    CONSTRAINT chk_users_pubkey_len CHECK (LENGTH(pubkey) = 32),
    -- agent owner is a user in the SAME community.
    FOREIGN KEY (community_id, agent_owner_pubkey)
        REFERENCES users (community_id, pubkey) ON DELETE SET NULL
);

-- NIP-05 handle and Okta id unique within a community, not globally.
CREATE UNIQUE INDEX idx_users_nip05 ON users (community_id, lower(nip05_handle))
    WHERE nip05_handle IS NOT NULL;
CREATE UNIQUE INDEX idx_users_okta ON users (community_id, okta_user_id)
    WHERE okta_user_id IS NOT NULL;

-- ── Events (partitioned by month on created_at) ──────────────────────────────
-- Conformance: "Channel-less global events and DMs". `community_id` leads the
-- PK and every hot-path index. Partition stays BY RANGE (created_at) — the
-- monthly partition manager is unchanged (Max's call, plan §5/Lane0 contract).
-- Cross-community dedup: same signed event may exist in two communities;
-- (community_id, created_at, id) dedupes within one, allows across.

CREATE TABLE events (
    community_id UUID NOT NULL REFERENCES communities(id),
    id          BYTEA NOT NULL,
    pubkey      BYTEA NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL,
    kind        INT NOT NULL,
    tags        JSONB NOT NULL,
    content     TEXT NOT NULL,
    -- Full-text search vector (Typesense → Postgres FTS). Generated/STORED so
    -- it is a single source of truth — no sidecar indexer to keep coherent
    -- (Quinn option A, Lane-0 call). 'simple' config = no stemming/stopwords,
    -- matching the existing substring-ish search semantics; the search lane can
    -- revisit the config behind evidence. Tenant scoping is by the
    -- community-leading btree filters BitmapAnd-ed with the GIN probe, so the
    -- GIN index itself stays the minimal `GIN (search_tsv)` (Max's caveat:
    -- avoid btree_gin unless EXPLAIN proves it buys something).
    -- Privacy: encrypted/private routing wrappers and p-gated membership notices
    -- must never be discoverable through NIP-50 full-text search. NULL tsvector
    -- never matches `@@`.
    -- URL/path punctuation is also split into words (migration 0077), so
    -- `callback` finds `https://x.org/auth/oidc/google/callback`; the original
    -- tokens stay for exact URL searches.
    -- Keep in sync with migrations (final state: 0001 + 0005 + 0014 + 0033 + 0062 + 0077).
    search_tsv  TSVECTOR GENERATED ALWAYS AS (
        CASE WHEN kind IN (1059, 30179, 30180, 30181, 30300, 30350, 30622, 44100, 44101, 44200) THEN NULL::tsvector
             ELSE to_tsvector('simple', content)
                  || to_tsvector('simple', regexp_replace(content, '[-/:._?=&#]+', ' ', 'g'))
        END
    ) STORED,
    -- Nullable since migration 0057: token-authenticated events are server
    -- stamped (Phase 0 still writes a 64-byte zero sentinel).
    sig         BYTEA,
    received_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    channel_sequence BIGINT NOT NULL DEFAULT 0 CHECK (channel_sequence >= 0),
    channel_id  UUID,
    deleted_at  TIMESTAMPTZ,
    d_tag       TEXT,
    not_before  BIGINT,
    delivered_at BIGINT,
    thread_root_sequence BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (community_id, created_at, id)
) PARTITION BY RANGE (created_at);

CREATE TABLE events_p_past PARTITION OF events
    FOR VALUES FROM (MINVALUE) TO ('2026-01-01');
CREATE TABLE events_p2026_01 PARTITION OF events
    FOR VALUES FROM ('2026-01-01') TO ('2026-02-01');
CREATE TABLE events_p2026_02 PARTITION OF events
    FOR VALUES FROM ('2026-02-01') TO ('2026-03-01');
CREATE TABLE events_p2026_03 PARTITION OF events
    FOR VALUES FROM ('2026-03-01') TO ('2026-04-01');
CREATE TABLE events_p2026_04 PARTITION OF events
    FOR VALUES FROM ('2026-04-01') TO ('2026-05-01');
CREATE TABLE events_p2026_05 PARTITION OF events
    FOR VALUES FROM ('2026-05-01') TO ('2026-06-01');
CREATE TABLE events_p2026_06 PARTITION OF events
    FOR VALUES FROM ('2026-06-01') TO ('2026-07-01');
CREATE TABLE events_p_future PARTITION OF events
    FOR VALUES FROM ('2026-07-01') TO (MAXVALUE);

-- Direct id lookup: the PK can't serve `WHERE id=$1` because created_at sits
-- between community_id and id. This index makes the scoped form
-- `WHERE community_id=$ AND id=$` index-served, not a partition scan.
CREATE INDEX idx_events_community_id ON events (community_id, id, created_at DESC);
-- Hot-path indexes, all community-leading.
CREATE INDEX idx_events_community_channel_created
    ON events (community_id, channel_id, created_at DESC, id);
CREATE INDEX idx_events_community_pubkey_kind_created
    ON events (community_id, pubkey, kind, created_at DESC, id);
CREATE INDEX idx_events_community_kind_created
    ON events (community_id, kind, created_at DESC, id);
CREATE INDEX idx_events_community_deleted ON events (community_id, deleted_at);
-- Addressable (replaceable) and NIP-33 parameterized lookups.
CREATE INDEX idx_events_addressable
    ON events (community_id, kind, pubkey, channel_id, deleted_at);
CREATE INDEX idx_events_parameterized
    ON events (community_id, kind, pubkey, d_tag, created_at DESC, id)
    WHERE d_tag IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX idx_events_not_before ON events (community_id, not_before)
    WHERE not_before IS NOT NULL AND deleted_at IS NULL AND delivered_at IS NULL;
-- Full-text search. Minimal GIN over the generated tsvector; community scoping
-- is supplied by the community-leading btree filters above (BitmapAnd), so this
-- stays a single-column GIN. The search lane confirms the final spelling with
-- EXPLAIN before its work lands (Quinn option A; Max's index-spelling caveat).
CREATE INDEX idx_events_search_tsv ON events USING GIN (search_tsv);

-- e-tag containment (`tags @> '[["e","<hex>"]]'`) for the aux closure and #e
-- reads. Mirrors migrations/0004; jsonb_path_ops supports exactly @>.
CREATE INDEX idx_events_tags_gin ON events USING GIN (tags jsonb_path_ops);

-- Unread counts from the channel acceptance sequence (migration 0068).
CREATE INDEX idx_events_community_channel_sequence
    ON events (community_id, channel_id, channel_sequence);

-- ── Event mentions ────────────────────────────────────────────────────────────
-- Conformance: "Channel-less global events and DMs" (#p fan-out). The join to
-- events MUST carry the community tuple (e.community_id = m.community_id AND
-- e.id = m.event_id) — bare e.id = m.event_id would leak cross-community
-- mentions (Max, verified at event.rs:222).

CREATE TABLE event_mentions (
    community_id        UUID NOT NULL REFERENCES communities(id),
    pubkey_hex          VARCHAR(64) NOT NULL,
    event_id            BYTEA NOT NULL,
    event_created_at    TIMESTAMPTZ NOT NULL,
    channel_id          UUID,
    event_kind          INT,
    PRIMARY KEY (community_id, pubkey_hex, event_id)
);

CREATE INDEX idx_event_mentions_pubkey_created
    ON event_mentions (community_id, pubkey_hex, event_created_at DESC);
CREATE INDEX idx_event_mentions_pubkey_kind_created
    ON event_mentions (community_id, pubkey_hex, event_kind, event_created_at DESC);

-- ── Subscriptions ─────────────────────────────────────────────────────────────
-- Conformance: "Mesh, agents, ACP/MCP, and CLI" (persisted subscriptions).

CREATE TABLE subscriptions (
    community_id        UUID NOT NULL REFERENCES communities(id),
    id                  VARCHAR(255) NOT NULL,
    owner_pubkey        BYTEA NOT NULL,
    filter_kinds        JSONB,
    filter_authors      JSONB,
    filter_channel_ids  JSONB,
    filter_since        TIMESTAMPTZ,
    filter_until        TIMESTAMPTZ,
    delivery_method     delivery_method NOT NULL DEFAULT 'webhook',
    delivery_url        TEXT,
    status              subscription_status NOT NULL DEFAULT 'active',
    pause_reason        pause_reason,
    delivered_count     BIGINT NOT NULL DEFAULT 0,
    error_count         BIGINT NOT NULL DEFAULT 0,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (community_id, id),
    FOREIGN KEY (community_id, owner_pubkey) REFERENCES users (community_id, pubkey)
);

-- ── Delivery log (partitioned by month on delivered_at) ──────────────────────
-- Conformance: subscription delivery audit. community_id carried for tenant
-- attribution; child of subscriptions.

CREATE TABLE delivery_log (
    community_id    UUID NOT NULL REFERENCES communities(id),
    id              BIGINT GENERATED ALWAYS AS IDENTITY,
    subscription_id VARCHAR(255),
    event_id        BYTEA,
    method          delivery_method,
    delivered_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    success         BOOLEAN,
    http_status     INT,
    error_message   TEXT,
    attempt_number  INT DEFAULT 1,
    PRIMARY KEY (delivered_at, id)
) PARTITION BY RANGE (delivered_at);

CREATE TABLE delivery_log_p_past PARTITION OF delivery_log
    FOR VALUES FROM (MINVALUE) TO ('2026-03-01');
CREATE TABLE delivery_log_p2026_03 PARTITION OF delivery_log
    FOR VALUES FROM ('2026-03-01') TO ('2026-04-01');
CREATE TABLE delivery_log_p2026_04 PARTITION OF delivery_log
    FOR VALUES FROM ('2026-04-01') TO ('2026-05-01');
CREATE TABLE delivery_log_p2026_05 PARTITION OF delivery_log
    FOR VALUES FROM ('2026-05-01') TO ('2026-06-01');
CREATE TABLE delivery_log_p2026_06 PARTITION OF delivery_log
    FOR VALUES FROM ('2026-06-01') TO ('2026-07-01');
CREATE TABLE delivery_log_p_future PARTITION OF delivery_log
    FOR VALUES FROM ('2026-07-01') TO (MAXVALUE);

CREATE INDEX idx_delivery_log_community_sub ON delivery_log (community_id, subscription_id);

-- ── Workflows ─────────────────────────────────────────────────────────────────
-- Conformance: "Workflows, runs, approvals, webhooks, schedules". Definition's
-- community fixed at create from req.community; runs/approvals inherit it.

CREATE TABLE workflows (
    community_id    UUID NOT NULL REFERENCES communities(id),
    id              UUID NOT NULL DEFAULT gen_random_uuid(),
    name            VARCHAR(255) NOT NULL,
    owner_pubkey    BYTEA NOT NULL,
    channel_id      UUID,
    definition      JSONB NOT NULL,
    definition_hash BYTEA NOT NULL,
    status          workflow_status NOT NULL DEFAULT 'active',
    enabled         BOOLEAN NOT NULL DEFAULT TRUE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (community_id, id),
    FOREIGN KEY (community_id, owner_pubkey) REFERENCES users (community_id, pubkey),
    FOREIGN KEY (community_id, channel_id) REFERENCES channels (community_id, id)
);

CREATE INDEX idx_workflows_channel_active ON workflows (community_id, channel_id, status, enabled);
-- Scheduler scans enabled schedule workflows; community_id returned per row so
-- side effects run under the owning tenant's context (Lane0 contract §4a.5).
CREATE INDEX idx_workflows_enabled ON workflows (enabled, status) WHERE enabled;

-- ── Workflow runs ─────────────────────────────────────────────────────────────

CREATE TABLE workflow_runs (
    community_id        UUID NOT NULL REFERENCES communities(id),
    id                  UUID NOT NULL DEFAULT gen_random_uuid(),
    workflow_id         UUID NOT NULL,
    status              run_status NOT NULL DEFAULT 'pending',
    trigger_event_id    BYTEA,
    current_step        INT NOT NULL DEFAULT 0,
    execution_trace     JSONB NOT NULL DEFAULT '[]',
    trigger_context     JSONB,
    started_at          TIMESTAMPTZ,
    completed_at        TIMESTAMPTZ,
    error_message       TEXT,
    error_code          TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (community_id, id),
    FOREIGN KEY (community_id, workflow_id)
        REFERENCES workflows (community_id, id) ON DELETE CASCADE
);

CREATE INDEX idx_workflow_runs_workflow ON workflow_runs (community_id, workflow_id);
CREATE INDEX idx_workflow_runs_status ON workflow_runs (community_id, status);

-- ── Workflow approvals ────────────────────────────────────────────────────────
-- token-hash lookup scoped: approval token grants cannot act on another
-- community's same hash (conformance).

CREATE TABLE workflow_approvals (
    community_id    UUID NOT NULL REFERENCES communities(id),
    token           BYTEA NOT NULL,
    workflow_id     UUID NOT NULL,
    run_id          UUID NOT NULL,
    step_id         VARCHAR(64) NOT NULL,
    step_index      INT NOT NULL,
    approver_spec   TEXT NOT NULL,
    status          approval_status NOT NULL DEFAULT 'pending',
    approver_pubkey BYTEA,
    note            TEXT,
    granted_at      TIMESTAMPTZ,
    denied_at       TIMESTAMPTZ,
    expires_at      TIMESTAMPTZ NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (community_id, token),
    FOREIGN KEY (community_id, workflow_id)
        REFERENCES workflows (community_id, id) ON DELETE CASCADE,
    FOREIGN KEY (community_id, run_id)
        REFERENCES workflow_runs (community_id, id) ON DELETE CASCADE
);

CREATE INDEX idx_workflow_approvals_workflow ON workflow_approvals (community_id, workflow_id);
CREATE INDEX idx_workflow_approvals_run ON workflow_approvals (community_id, run_id);
CREATE INDEX idx_workflow_approvals_status ON workflow_approvals (community_id, status);

-- ── Scheduled workflow fires (cron claim) ─────────────────────────────────────
-- Plan §5: the at-most-once cron fire claim. UNIQUE (community_id, workflow_id,
-- scheduled_for) — only the pod that wins the claim insert creates the run.
-- Restart-safe (DB-durable). community is server provenance: the scheduler passes
-- workflow.community_id from list_all_enabled_workflows(), never a client input.
-- workflow_id is NOT globally unique under the (community_id, id) workflow key, so
-- the claim binds both community and id explicitly rather than resolving from id.
-- workflow_run_id links the won claim to the run it created (audit; NULL until the
-- post-insert attach, and stays NULL if run creation failed after a won claim).
-- The FK to workflow_runs uses NO ACTION (not SET NULL): community_id is shared
-- with the claim PK and is NOT NULL, so SET NULL is unimplementable here; a future
-- delete of a still-linked run is blocked rather than orphaning the at-most-once
-- claim row. workflow_runs are not pruned today, so this is a guardrail, not a path.

CREATE TABLE scheduled_workflow_fires (
    community_id    UUID NOT NULL REFERENCES communities(id),
    workflow_id     UUID NOT NULL,
    scheduled_for   TIMESTAMPTZ NOT NULL,
    claimed_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    workflow_run_id UUID,
    PRIMARY KEY (community_id, workflow_id, scheduled_for),
    FOREIGN KEY (community_id, workflow_id)
        REFERENCES workflows (community_id, id) ON DELETE CASCADE,
    FOREIGN KEY (community_id, workflow_run_id)
        REFERENCES workflow_runs (community_id, id) ON DELETE NO ACTION
);

-- The interval anchor reads MAX(scheduled_for) per workflow; the janitor prunes
-- by claimed_at globally (operator concern). See plan §5 retention coupling.
CREATE INDEX idx_scheduled_fires_claimed_at ON scheduled_workflow_fires (claimed_at);

-- ── API tokens ────────────────────────────────────────────────────────────────
-- Conformance: "API tokens and NIP-98 replay". token_hash uniqueness scoped to
-- (community_id, token_hash); channel claims reference channels in same community.

CREATE TABLE api_tokens (
    community_id        UUID NOT NULL REFERENCES communities(id),
    id                  UUID NOT NULL DEFAULT gen_random_uuid(),
    token_hash          BYTEA NOT NULL,
    owner_pubkey        BYTEA NOT NULL,
    name                VARCHAR(255) NOT NULL,
    scopes              JSONB NOT NULL,
    channel_ids         JSONB,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at          TIMESTAMPTZ,
    last_used_at        TIMESTAMPTZ,
    revoked_at          TIMESTAMPTZ,
    revoked_by          BYTEA,
    created_by_self_mint BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (community_id, id),
    FOREIGN KEY (community_id, owner_pubkey) REFERENCES users (community_id, pubkey),
    CONSTRAINT chk_api_tokens_hash_len CHECK (LENGTH(token_hash) = 32)
);

CREATE UNIQUE INDEX idx_api_tokens_hash ON api_tokens (community_id, token_hash);

-- ── Rate limit violations ─────────────────────────────────────────────────────
-- OPERATOR-GLOBAL: a deployment-health / abuse table, never tenant-observable.
-- Listed in the lint allowlist. Carries community_id as an attribution label
-- only (nullable, no uniqueness over it).

CREATE TABLE rate_limit_violations (
    id              BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    community_id    UUID,
    pubkey          BYTEA,
    violation_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    limit_type      VARCHAR(64),
    limit_value     INT,
    actual_value    INT,
    action_taken    VARCHAR(64)
);

-- ── Thread metadata ───────────────────────────────────────────────────────────
-- Conformance: thread lookups filter by community before event matching.

CREATE TABLE thread_metadata (
    community_id            UUID NOT NULL REFERENCES communities(id),
    event_created_at        TIMESTAMPTZ NOT NULL,
    event_id                BYTEA NOT NULL,
    channel_id              UUID NOT NULL,
    parent_event_id         BYTEA,
    parent_event_created_at TIMESTAMPTZ,
    root_event_id           BYTEA,
    root_event_created_at   TIMESTAMPTZ,
    depth                   INT NOT NULL DEFAULT 0,
    reply_count             INT NOT NULL DEFAULT 0,
    descendant_count        INT NOT NULL DEFAULT 0,
    last_reply_at           TIMESTAMPTZ,
    broadcast               BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (community_id, event_created_at, event_id),
    FOREIGN KEY (community_id, channel_id) REFERENCES channels (community_id, id)
);

CREATE INDEX idx_thread_metadata_parent ON thread_metadata (community_id, parent_event_id);
CREATE INDEX idx_thread_metadata_root ON thread_metadata (community_id, root_event_id);
CREATE INDEX idx_thread_metadata_window
    ON thread_metadata (community_id, root_event_id, event_created_at DESC, event_id ASC);
CREATE INDEX idx_thread_metadata_channel_depth
    ON thread_metadata (community_id, channel_id, depth, event_created_at);
CREATE INDEX idx_thread_metadata_event_id ON thread_metadata (community_id, event_id);
CREATE INDEX idx_thread_metadata_inbox
    ON thread_metadata (community_id, channel_id, last_reply_at DESC, event_id DESC)
    WHERE depth = 0 AND reply_count > 0;

-- ── Thread follows and thread state (migration 0068) ─────────────────────────
-- Server-side chat state. Rows exist only for channel members. Plain form so
-- every device of a user sees the same list; purged with the community.

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
    -- Where read_seq came from (migration 0069): 'backfill', 'user', or NULL.
    read_origin    TEXT CHECK (read_origin IN ('backfill', 'user')),
    -- Stored unread counts, capped at 100 (migration 0072).
    unread_count    INTEGER NOT NULL DEFAULT 0,
    unread_mentions INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (community_id, root_event_id, pubkey),
    FOREIGN KEY (community_id, channel_id)
        REFERENCES channels (community_id, id) ON DELETE CASCADE
);

CREATE INDEX idx_thread_follows_pubkey ON thread_follows (community_id, pubkey);

-- Separate from thread_metadata because the root's depth-0 row exists only
-- after the first reply. last_seq/last_at ignore close and reopen markers.
CREATE TABLE thread_state (
    community_id   UUID NOT NULL REFERENCES communities(id),
    root_event_id  BYTEA NOT NULL CHECK (length(root_event_id) = 32),
    channel_id     UUID NOT NULL,
    last_seq       BIGINT,
    last_at        TIMESTAMPTZ,
    closed_at      TIMESTAMPTZ,
    closed_by      BYTEA,
    -- Latest live reply (not a marker) and newest thread name (migration 0072).
    last_event_id  BYTEA,
    name           TEXT,
    PRIMARY KEY (community_id, root_event_id),
    FOREIGN KEY (community_id, channel_id)
        REFERENCES channels (community_id, id) ON DELETE CASCADE
);

-- ── Blocks ────────────────────────────────────────────────────────────────────
-- Silent blocks (migration 0071): `blocker` no longer receives `blocked`'s DM
-- events (live, history, unread, push). Private to the blocker.

CREATE TABLE user_blocks (
    community_id UUID NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    blocker      BYTEA NOT NULL CHECK (length(blocker) = 32),
    blocked      BYTEA NOT NULL CHECK (length(blocked) = 32),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, blocker, blocked),
    CONSTRAINT user_blocks_not_self CHECK (blocker <> blocked)
);
CREATE INDEX idx_user_blocks_blocked ON user_blocks (community_id, blocked);

-- Block periods (migration 0073): one row per block's time span, open while
-- the block is on. Stored 1:1 DM messages received during a period are hidden
-- from the blocker, also after unblocking.
CREATE TABLE user_block_periods (
    community_id UUID NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    blocker      BYTEA NOT NULL CHECK (length(blocker) = 32),
    blocked      BYTEA NOT NULL CHECK (length(blocked) = 32),
    started_at   TIMESTAMPTZ NOT NULL,
    ended_at     TIMESTAMPTZ,
    PRIMARY KEY (community_id, blocker, blocked, started_at),
    CONSTRAINT user_block_periods_not_self CHECK (blocker <> blocked),
    CONSTRAINT user_block_periods_ordered CHECK (ended_at IS NULL OR ended_at >= started_at)
);
CREATE UNIQUE INDEX user_block_periods_open
    ON user_block_periods (community_id, blocker, blocked) WHERE ended_at IS NULL;

-- ── Media access ──────────────────────────────────────────────────────────────
-- Attachment permissions (migration 0074): a blob is readable by its uploaders
-- and by whoever can currently read an event that references it. NULL
-- `channel_id` means a community-wide event (profile, emoji set, shared persona).

CREATE TABLE media_uploads (
    community_id UUID NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    sha256       TEXT NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    uploader     BYTEA NOT NULL CHECK (length(uploader) = 32),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, sha256, uploader)
);

CREATE TABLE media_references (
    community_id     UUID NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    sha256           TEXT NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    event_id         BYTEA NOT NULL CHECK (length(event_id) = 32),
    event_created_at TIMESTAMPTZ NOT NULL,
    channel_id       UUID,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, sha256, event_id)
);

-- ── Groups ────────────────────────────────────────────────────────────────────
-- Invite links, "who can add me" and invitations (migration 0075).

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

CREATE TABLE group_add_settings (
    community_id UUID NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    pubkey       BYTEA NOT NULL CHECK (length(pubkey) = 32),
    policy       TEXT NOT NULL DEFAULT 'friends' CHECK (policy IN ('friends', 'everyone')),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, pubkey)
);

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

-- ── Reactions ─────────────────────────────────────────────────────────────────
-- Conformance: reactions filter by community before event/pubkey matching.

CREATE TABLE reactions (
    community_id        UUID NOT NULL REFERENCES communities(id),
    event_created_at    TIMESTAMPTZ NOT NULL,
    event_id            BYTEA NOT NULL,
    pubkey              BYTEA NOT NULL,
    emoji               VARCHAR(66) NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    removed_at          TIMESTAMPTZ,
    reaction_event_id   BYTEA,
    PRIMARY KEY (community_id, event_created_at, event_id, pubkey, emoji)
);

CREATE INDEX idx_reactions_event ON reactions (community_id, event_id, event_created_at);
CREATE INDEX idx_reactions_pubkey ON reactions (community_id, pubkey);
-- A reaction's source event id is unique within a community.
CREATE UNIQUE INDEX idx_reactions_source_event ON reactions (community_id, reaction_event_id)
    WHERE reaction_event_id IS NOT NULL;

-- ── Pubkey allowlist ──────────────────────────────────────────────────────────
-- Conformance: "Relay membership, pubkey allowlist, archived identities".
-- PK becomes (community_id, pubkey).

CREATE TABLE pubkey_allowlist (
    community_id UUID NOT NULL REFERENCES communities(id),
    pubkey      BYTEA NOT NULL,
    added_by    BYTEA,
    added_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    note        TEXT,
    PRIMARY KEY (community_id, pubkey)
);

-- ── Relay members (NIP-43) ────────────────────────────────────────────────────
-- Conformance: membership gate, community-scoped. pubkey stored as hex TEXT
-- (unchanged wire form). PK (community_id, pubkey).

CREATE TABLE relay_members (
    community_id UUID NOT NULL REFERENCES communities(id),
    pubkey      TEXT NOT NULL,
    role        TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
    added_by    TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, pubkey)
);

CREATE INDEX idx_relay_members_role ON relay_members (community_id, role);

-- ── Join policy acceptances ──────────────────────────────────────────────────
-- Durable evidence of the policy version accepted when an invite claim grants
-- relay membership. The composite foreign key keeps evidence bound to a live
-- member in the same community and removes it with that membership.

CREATE TABLE join_policy_acceptances (
    community_id UUID NOT NULL,
    pubkey TEXT NOT NULL,
    policy_version TEXT NOT NULL CHECK (length(policy_version) = 64),
    accepted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, pubkey, policy_version),
    FOREIGN KEY (community_id, pubkey)
        REFERENCES relay_members (community_id, pubkey) ON DELETE CASCADE
);

-- ── Relay invites (use-limited invite links) ──────────────────────────────────
-- Conformance: durable invite records for atomic redemption, community-scoped.
-- Stores only SHA-256(code) as 32-byte BYTEA; never the reusable bearer code.
-- PK and UNIQUE both lead with community_id. max_uses NULL = unlimited.

CREATE TABLE relay_invites (
    community_id  UUID        NOT NULL REFERENCES communities(id),
    id           UUID        NOT NULL DEFAULT gen_random_uuid(),
    token_hash   BYTEA       NOT NULL CHECK (length(token_hash) = 32),
    role         TEXT        NOT NULL DEFAULT 'member' CHECK (role = 'member'),
    max_uses     INTEGER     CHECK (max_uses BETWEEN 1 AND 10000),
    use_count    INTEGER     NOT NULL DEFAULT 0 CHECK (use_count >= 0),
    expires_at   TIMESTAMPTZ NOT NULL,
    created_by   TEXT        NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, id),
    UNIQUE (community_id, token_hash),
    CHECK (max_uses IS NULL OR use_count <= max_uses)
);

CREATE INDEX relay_invites_expires_at_idx ON relay_invites (expires_at);

-- ── Archived identities (NIP-IA) ──────────────────────────────────────────────
-- Conformance: archive cannot hide a key in another community. PK scoped.

CREATE TABLE archived_identities (
    community_id      UUID NOT NULL REFERENCES communities(id),
    pubkey            TEXT NOT NULL,
    consent_path      TEXT NOT NULL CHECK (consent_path IN ('self', 'owner', 'admin')),
    actor             TEXT NOT NULL,
    reason            TEXT,
    replaced_by       TEXT,
    request_event_id  TEXT NOT NULL,
    archived_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, pubkey)
);

-- ── Audit log ─────────────────────────────────────────────────────────────────
-- Conformance: "Audit log and observability". Per-community hash chain:
-- uniqueness (community_id, seq) and (community_id, hash). One chain per tenant.
-- (Lane Audit/Dawn builds the chain logic; Lane 0 fixes the scoped schema.)

CREATE TABLE audit_log (
    community_id    UUID NOT NULL REFERENCES communities(id),
    seq             BIGINT NOT NULL,
    hash_version    SMALLINT NOT NULL DEFAULT 1 CHECK (hash_version IN (1, 2)),
    hash            BYTEA NOT NULL,
    prev_hash       BYTEA,
    action          VARCHAR(64) NOT NULL,
    actor_pubkey    BYTEA,
    object_id       TEXT,
    detail          JSONB,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (community_id, seq)
);

CREATE UNIQUE INDEX idx_audit_log_hash ON audit_log (community_id, hash);

-- ── NIP-56 reports (kind:1984 ingest) ─────────────────────────────────────────
-- One row per accepted report event. Reports are signals, never triggers:
-- nothing auto-actions on them (NIP-56). Reporter identity is visible to
-- moderators in the queue but never revealed to the reported author.

CREATE TABLE moderation_reports (
    community_id        UUID NOT NULL REFERENCES communities(id),
    id                  UUID NOT NULL DEFAULT gen_random_uuid(),
    -- The signed kind:1984 event id (stored for audit/idempotency).
    report_event_id     BYTEA NOT NULL CHECK (length(report_event_id) = 32),
    reporter_pubkey     BYTEA NOT NULL CHECK (length(reporter_pubkey) = 32),
    -- What was reported. Exactly one target class per row (CHECK-enforced below).
    target_kind         TEXT NOT NULL CHECK (target_kind IN ('event', 'pubkey', 'blob')),
    target_event_id     BYTEA CHECK (target_event_id IS NULL OR length(target_event_id) = 32),
    target_pubkey       BYTEA CHECK (target_pubkey IS NULL OR length(target_pubkey) = 32),
    target_blob_sha256  BYTEA CHECK (target_blob_sha256 IS NULL OR length(target_blob_sha256) = 32),
    -- Channel inferred from an in-tenant target event row, when resolvable.
    channel_id          UUID,
    -- NIP-56 report type: illegal|nudity|malware|spam|impersonation|profanity|other.
    report_type         TEXT NOT NULL,
    -- Reporter's optional free-text context (mod-queue-only; never public).
    note                TEXT,
    status              TEXT NOT NULL DEFAULT 'open'
                        CHECK (status IN ('open', 'processing', 'resolved', 'dismissed', 'escalated')),
    -- Non-null when status='processing': the relay_admin_actions row that claimed this report.
    active_action_id    UUID,
    resolved_by         BYTEA,
    resolved_at         TIMESTAMPTZ,
    -- moderation_actions row that resolved this report, if any.
    action_id           UUID,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, id),
    -- Exactly one target class per row: target_kind is authoritative and the
    -- matching column (only) is populated. Queue/action code never guesses.
    CHECK (
        (target_kind = 'event'  AND target_event_id IS NOT NULL AND target_pubkey IS NULL     AND target_blob_sha256 IS NULL) OR
        (target_kind = 'pubkey' AND target_event_id IS NULL     AND target_pubkey IS NOT NULL AND target_blob_sha256 IS NULL) OR
        (target_kind = 'blob'   AND target_event_id IS NULL     AND target_pubkey IS NULL     AND target_blob_sha256 IS NOT NULL)
    ),
    -- Same-community channel provenance (channels are soft-deleted, never
    -- hard-deleted, so this FK cannot dangle).
    FOREIGN KEY (community_id, channel_id) REFERENCES channels (community_id, id)
);

-- Queue reads: open reports, newest first, per community.
CREATE INDEX idx_moderation_reports_status
    ON moderation_reports (community_id, status, created_at DESC);
-- Group-by-target for triage aggregation.
CREATE INDEX idx_moderation_reports_target_event
    ON moderation_reports (community_id, target_event_id)
    WHERE target_event_id IS NOT NULL;
CREATE INDEX idx_moderation_reports_target_pubkey
    ON moderation_reports (community_id, target_pubkey)
    WHERE target_pubkey IS NOT NULL;
-- Idempotency: one row per report event per community.
CREATE UNIQUE INDEX idx_moderation_reports_event
    ON moderation_reports (community_id, report_event_id);

-- ── Bans + timeouts (one restriction row per member) ──────────────────────────
-- Ban = connection block, enforced at the NIP-42 auth seam
-- ("blocked: you are banned from this community") + join/ingest surfaces.
-- Timeout = write-block only ("restricted: you are timed out until <ts>").
-- A row may be ban-only, timeout-only, or both over its lifetime.

CREATE TABLE community_bans (
    community_id    UUID NOT NULL REFERENCES communities(id),
    pubkey          BYTEA NOT NULL CHECK (length(pubkey) = 32),
    banned          BOOLEAN NOT NULL DEFAULT false,
    -- NULL + banned=true ⇒ permanent.
    ban_expires_at  TIMESTAMPTZ,
    ban_reason      TEXT,
    -- Write-block until this timestamp; NULL or past ⇒ not timed out.
    muted_until     TIMESTAMPTZ,
    mute_reason     TEXT,
    -- Moderator who last modified this row.
    actor_pubkey    BYTEA NOT NULL CHECK (length(actor_pubkey) = 32),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, pubkey)
);

-- ── Moderation audit ──────────────────────────────────────────────────────────
-- One row per accepted moderation action. Full detail (reporter identities,
-- private reasons, matched NIP-OA principal) stays mod/audit-only; the public
-- tombstone carries only action_id + reason_code + sanitized public_reason.

CREATE TABLE moderation_actions (
    community_id    UUID NOT NULL REFERENCES communities(id),
    id              UUID NOT NULL DEFAULT gen_random_uuid(),
    actor_pubkey    BYTEA NOT NULL CHECK (length(actor_pubkey) = 32),
    action          TEXT NOT NULL CHECK (action IN (
                        'delete_message', 'kick', 'ban', 'unban',
                        'timeout', 'untimeout', 'dismiss_report', 'escalate',
                        'resolve:delete', 'resolve:kick', 'resolve:ban',
                        'resolve:timeout')),
    target_pubkey   BYTEA CHECK (target_pubkey IS NULL OR length(target_pubkey) = 32),
    target_event_id BYTEA CHECK (target_event_id IS NULL OR length(target_event_id) = 32),
    channel_id      UUID,
    -- Machine-readable rule/reason code (e.g. "spam", "community_rule_3").
    reason_code     TEXT,
    -- Sanitized, safe for the public tombstone.
    public_reason   TEXT,
    -- Mod-only context; never leaves the audit surface.
    private_reason  TEXT,
    -- NIP-OA: which principal matched a ban ('self' | 'owner'); audit-only,
    -- the client never learns which.
    matched_principal TEXT CHECK (matched_principal IS NULL OR matched_principal IN ('self', 'owner')),
    -- Deployment authority type for HTTP-initiated actions.
    actor_authority   TEXT NOT NULL DEFAULT 'community'
                      CHECK (actor_authority IN ('community', 'relay_operator', 'relay_moderator')),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, id),
    FOREIGN KEY (community_id, channel_id) REFERENCES channels (community_id, id)
);

CREATE INDEX idx_moderation_actions_created
    ON moderation_actions (community_id, created_at DESC);
CREATE INDEX idx_moderation_actions_target_pubkey
    ON moderation_actions (community_id, target_pubkey)
    WHERE target_pubkey IS NOT NULL;

-- Same-community resolution provenance: a report can only be resolved by an
-- action row in its own community. Added after moderation_actions exists.
ALTER TABLE moderation_reports
    ADD FOREIGN KEY (community_id, action_id)
    REFERENCES moderation_actions (community_id, id);

-- ── Lint allowlist registry ───────────────────────────────────────────────────
-- The explicit registry of tables that are deliberately operator-global (NOT
-- tenant-scoped). The migration-lint harness reads this: any table NOT listed
-- here MUST carry a NOT NULL community_id and lead its uniques with it. Making
-- the allowlist a DB table (not a hard-coded list in the linter) keeps the
-- registry next to the schema it governs and reviewable in one migration diff.

CREATE TABLE _operator_global_tables (
    table_name  TEXT PRIMARY KEY,
    reason      TEXT NOT NULL
);

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('communities',           'the tenant registry itself; id IS the community key'),
    ('rate_limit_violations', 'deployment abuse/health; never tenant-observable; community_id is an attribution label only'),
    ('_operator_global_tables', 'the registry table itself');

-- ── Additive tenant tables represented in migrations 0002/0007/0017 ──────────
-- Keep desired-state schema parity with the embedded SQLx migration path.
CREATE TABLE git_repo_names (
    community_id  UUID NOT NULL REFERENCES communities(id),
    repo_id       TEXT NOT NULL,
    owner_pubkey  TEXT NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, repo_id)
);
CREATE INDEX idx_git_repo_names_owner ON git_repo_names (community_id, owner_pubkey);

CREATE TABLE parameterized_event_watermarks (
    community_id  UUID NOT NULL REFERENCES communities(id),
    kind          INT NOT NULL,
    pubkey        BYTEA NOT NULL,
    d_tag         TEXT NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL,
    event_id      BYTEA NOT NULL,
    PRIMARY KEY (community_id, kind, pubkey, d_tag)
);
CREATE INDEX idx_event_mentions_community_event
    ON event_mentions (community_id, event_id);

CREATE TABLE product_feedback (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    community_id UUID REFERENCES communities(id) ON DELETE SET NULL,
    event_id BYTEA NOT NULL CHECK (length(event_id) = 32),
    submitter_pubkey BYTEA NOT NULL CHECK (length(submitter_pubkey) = 32),
    category TEXT CHECK (category IN ('bug', 'praise', 'needs-work')),
    body TEXT NOT NULL CHECK (length(btrim(body)) > 0),
    tags JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(tags) = 'array'),
    event_created_at TIMESTAMPTZ NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Operator-managed lifecycle status.
    status TEXT NOT NULL DEFAULT 'new' CHECK (status IN ('new', 'reviewed', 'archived')),
    UNIQUE (event_id)
);
CREATE INDEX idx_product_feedback_received
    ON product_feedback (received_at DESC, id);
CREATE INDEX idx_product_feedback_community_received
    ON product_feedback (community_id, received_at DESC, id);
INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('product_feedback', 'deployment product inbox; community_id is provenance only');
-- NIP-PL effective lease state and durable wake outbox. Every key is led by
-- community_id: client-provided origin is confirmation only, never routing.
CREATE TABLE push_leases (
    community_id UUID NOT NULL REFERENCES communities(id),
    author BYTEA NOT NULL CHECK (length(author) = 32),
    installation_id TEXT NOT NULL CHECK (octet_length(installation_id) BETWEEN 1 AND 64),
    source_event_id BYTEA NOT NULL CHECK (length(source_event_id) = 32),
    source_created_at BIGINT NOT NULL,
    generation BIGINT NOT NULL CHECK (generation > 0),
    active BOOLEAN NOT NULL,
    endpoint_enabled BOOLEAN NOT NULL DEFAULT true,
    app_profile TEXT,
    endpoint_hash BYTEA CHECK (endpoint_hash IS NULL OR length(endpoint_hash) = 32),
    endpoint_grant TEXT,
    max_class TEXT CHECK (max_class IS NULL OR max_class IN ('silent','default','time_sensitive','urgent')),
    subscriptions JSONB,
    expires_at BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, author, installation_id),
    UNIQUE (community_id, source_event_id),
    CHECK ((active AND app_profile IS NOT NULL AND endpoint_hash IS NOT NULL AND endpoint_grant IS NOT NULL AND max_class IS NOT NULL AND subscriptions IS NOT NULL)
        OR (NOT active AND app_profile IS NULL AND endpoint_hash IS NULL AND endpoint_grant IS NULL AND max_class IS NULL AND subscriptions IS NULL))
);
CREATE UNIQUE INDEX push_leases_endpoint_unique
    ON push_leases (community_id, author, app_profile, endpoint_hash)
    WHERE active;
CREATE INDEX push_leases_expiry ON push_leases (community_id, expires_at) WHERE active;

CREATE TABLE push_wake_outbox (
    community_id UUID NOT NULL REFERENCES communities(id),
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    author BYTEA NOT NULL CHECK (length(author) = 32),
    installation_id TEXT NOT NULL,
    lease_generation BIGINT NOT NULL CHECK (lease_generation > 0),
    endpoint_hash BYTEA NOT NULL CHECK (length(endpoint_hash) = 32),
    event_id BYTEA NOT NULL CHECK (length(event_id) = 32),
    class TEXT NOT NULL CHECK (class IN ('silent','default','time_sensitive','urgent')),
    expires_at BIGINT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending','sending','delivered','failed')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    lease_until TIMESTAMPTZ,
    claim_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, id),
    FOREIGN KEY (community_id, author, installation_id)
        REFERENCES push_leases (community_id, author, installation_id),
    UNIQUE (community_id, endpoint_hash, event_id)
);
CREATE INDEX push_wake_outbox_due
    ON push_wake_outbox (community_id, next_attempt_at) WHERE state = 'pending';
CREATE INDEX push_wake_outbox_recovery
    ON push_wake_outbox (community_id, lease_until) WHERE state = 'sending';
-- Durable event-to-push matching follower. The trigger runs in the event insert
-- transaction, so every accepted persistent event has a crash-safe match job and
-- rejected/rolled-back events never do. Processing is idempotent through the
-- push_wake_outbox endpoint/event unique key.
CREATE TABLE push_match_queue (
    community_id UUID NOT NULL REFERENCES communities(id),
    event_id BYTEA NOT NULL CHECK (length(event_id) = 32),
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending','matching')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    lease_until TIMESTAMPTZ,
    claim_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, event_id)
);
CREATE INDEX push_match_queue_due
    ON push_match_queue (next_attempt_at, created_at) WHERE state = 'pending';
CREATE INDEX push_match_queue_recovery
    ON push_match_queue (lease_until) WHERE state = 'matching';

-- T1b push gate (keep in sync with migrations/0023). Enqueue only when the
-- community has an active, endpoint-enabled, unexpired lease; the shared
-- advisory lock pairs with the exclusive lock taken by lease activations
-- (crates/kuzz-db/src/push.rs) to close the lost-wake race.
CREATE FUNCTION enqueue_push_match_job() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    -- Keep this allowlist identical to the relay's validated NIP-PL descriptor.
    -- Centralizing it on the events table covers every durable producer,
    -- including internal paths that bypass live dispatch.
    IF NEW.kind IN (9, 40002, 45001, 45003) THEN
        PERFORM pg_advisory_xact_lock_shared(
            hashtextextended('kuzz_push_gate:' || NEW.community_id::text, 0));
        IF EXISTS (
            SELECT 1 FROM push_leases
            WHERE community_id = NEW.community_id
              AND active
              AND endpoint_enabled
              AND expires_at > EXTRACT(EPOCH FROM now())::bigint
        ) THEN
            INSERT INTO push_match_queue (community_id, event_id)
            VALUES (NEW.community_id, NEW.id)
            ON CONFLICT DO NOTHING;
        END IF;
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER events_enqueue_push_match
AFTER INSERT ON events
FOR EACH ROW EXECUTE FUNCTION enqueue_push_match_job();

-- Channel TTL refresh (keep in sync with migrations/0024). Runs deferred, in
-- the transaction that makes a channel-scoped event durable, so a TTL
-- transition committed while ingest was in flight is never missed. The
-- per-channel advisory lock is SHARED here — permanent-channel commits admit
-- each other — and taken EXCLUSIVE by TTL transitions (update_channel in
-- crates/kuzz-db/src/channel.rs), which forces the same total order the
-- 0022 row lock provided without serializing the hot path.
CREATE FUNCTION refresh_channel_ttl_after_event_insert() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    channel_ttl INTEGER;
BEGIN
    -- Kind 9007 creates the channel and initializes its deadline itself.
    IF NEW.channel_id IS NOT NULL AND NEW.kind <> 9007 THEN
        BEGIN
            PERFORM pg_advisory_xact_lock_shared(hashtextextended(
                'kuzz_channel_ttl:' || NEW.community_id::text || ':' || NEW.channel_id::text, 0));

            SELECT ttl_seconds INTO channel_ttl
            FROM channels
            WHERE community_id = NEW.community_id AND id = NEW.channel_id;

            IF channel_ttl IS NOT NULL THEN
                UPDATE channels
                SET ttl_deadline = clock_timestamp() + make_interval(secs => ttl_seconds)
                WHERE community_id = NEW.community_id
                  AND id = NEW.channel_id
                  AND ttl_seconds IS NOT NULL
                  AND archived_at IS NULL
                  AND deleted_at IS NULL;
            END IF;
        EXCEPTION WHEN OTHERS THEN
            -- Preserve the existing best-effort contract: a TTL refresh failure
            -- must not reject an otherwise valid durable event.
            RAISE WARNING 'channel TTL refresh failed for community %, channel %: %',
                NEW.community_id, NEW.channel_id, SQLERRM;
        END;
    END IF;
    RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER events_refresh_channel_ttl
AFTER INSERT ON events
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION refresh_channel_ttl_after_event_insert();

-- Channel roster snapshot fence (keep in sync with migrations/0032).
-- Prevent mixed-version relay pods from publishing a stale NIP-29 member
-- snapshot after a newer canonical roster has been committed.
--
-- Old binaries already serialize kind 39002 replacement on the replacement
-- advisory key. This trigger adds the channel-membership key at INSERT time,
-- after that canonical key, and validates every p tag against the current
-- active membership set and roles. New binaries take both keys in the same
-- order before capture and replacement. Thus old and new writers remain
-- compatible during a rolling deploy.
CREATE OR REPLACE FUNCTION guard_channel_roster_snapshot()
RETURNS TRIGGER AS $$
DECLARE
    canonical_members TEXT[];
    snapshot_members TEXT[];
BEGIN
    IF NEW.kind <> 39002 OR NEW.channel_id IS NULL THEN
        RETURN NEW;
    END IF;

    PERFORM pg_advisory_xact_lock(hashtextextended(
        'kuzz_channel_membership:' || NEW.community_id::text || ':' || NEW.channel_id::text,
        0
    ));

    SELECT COALESCE(
               array_agg(encode(cm.pubkey, 'hex') || ':' || cm.role::text ORDER BY cm.pubkey),
               ARRAY[]::TEXT[]
           )
      INTO canonical_members
      FROM channel_members cm
     WHERE cm.community_id = NEW.community_id
       AND cm.channel_id = NEW.channel_id
       AND cm.removed_at IS NULL;

    -- A roster is canonical only when every p tag uses the emitted four-field
    -- shape, contains a 32-byte hex pubkey and valid authoritative role, has no
    -- duplicate members, and exactly matches the active membership rows.
    IF EXISTS (
        SELECT 1
          FROM jsonb_array_elements(NEW.tags) AS roster_tag(tag_json)
         WHERE roster_tag.tag_json->>0 = 'p'
           AND (
               jsonb_array_length(roster_tag.tag_json) <> 4
               OR COALESCE(roster_tag.tag_json->>1, '') !~ '^[0-9a-fA-F]{64}$'
               OR roster_tag.tag_json->>2 <> ''
               OR COALESCE(roster_tag.tag_json->>3, '') NOT IN ('owner', 'admin', 'bot', 'member', 'guest')
           )
    ) THEN
        RAISE EXCEPTION 'kind 39002 roster contains an invalid p tag'
            USING ERRCODE = '23514';
    END IF;

    SELECT COALESCE(
               array_agg(
                   lower((roster_tag.tag_json->>1)) || ':' || (roster_tag.tag_json->>3)
                   ORDER BY decode((roster_tag.tag_json->>1), 'hex')
               ),
               ARRAY[]::TEXT[]
           )
      INTO snapshot_members
      FROM jsonb_array_elements(NEW.tags) AS roster_tag(tag_json)
     WHERE roster_tag.tag_json->>0 = 'p';

    IF snapshot_members IS DISTINCT FROM canonical_members THEN
        RAISE EXCEPTION 'kind 39002 roster does not match canonical channel membership'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_events_guard_channel_roster_snapshot ON events;
CREATE TRIGGER trg_events_guard_channel_roster_snapshot
    BEFORE INSERT ON events
    FOR EACH ROW EXECUTE FUNCTION guard_channel_roster_snapshot();


-- Replica-fence floor guard (keep in sync with migrations/0021). A deferred
-- constraint trigger re-checks, inside COMMIT processing, that channel-bearing
-- event rows are no older than `kuzz.created_at_floor` seconds before commit
-- time (clock_timestamp(), NOT the transaction-frozen now()). This turns the
-- relay's ingest-time created_at envelope into a commit-time storage
-- invariant, which is what lets keyset-cursor pages below the replica fence
-- be served by a read replica without holes. Enforcement is armed per session
-- via the GUC (set by the relay's writer pool on connect); sessions without
-- the GUC (pg_restore, manual backfills) bypass it and must hold the replica
-- fence closed for their duration. The only structural exemption is
-- channel_id IS NULL: those rows never appear in keyset-paged windows.
CREATE FUNCTION events_created_at_floor_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    floor_secs numeric := nullif(current_setting('kuzz.created_at_floor', true), '')::numeric;
BEGIN
    IF floor_secs IS NOT NULL
       AND floor_secs > 0
       AND NEW.channel_id IS NOT NULL
       AND NEW.created_at < clock_timestamp() - make_interval(secs => floor_secs)
    THEN
        RAISE EXCEPTION
            'events.created_at % is more than % s before commit time %; below the replica-fence floor',
            NEW.created_at, floor_secs, clock_timestamp()
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NULL;
END
$$;

-- INSERT OR UPDATE OF: an UPDATE can move a previously exempt row into the
-- guarded set (channel_id NULL -> NOT NULL) or move a channel row's
-- created_at below the fence, so both mutation paths re-run the guard on the
-- NEW row. A created_at rewrite that crosses partition bounds runs as
-- DELETE + INSERT and hits the cloned AFTER INSERT guard on the destination
-- partition; an in-partition rewrite fires the UPDATE OF arm.
CREATE CONSTRAINT TRIGGER events_created_at_floor
    AFTER INSERT OR UPDATE OF created_at, channel_id ON events
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION events_created_at_floor_guard();

-- Durable, deployment-global authority for the public NIP-PL push gateway.
-- This state is intentionally outside relay community tenancy: installations
-- delegate to relay signing keys and may authorize multiple relay deployments.
CREATE TABLE push_gateway_challenges (
    id UUID PRIMARY KEY,
    challenge_hash BYTEA NOT NULL CHECK (length(challenge_hash) = 32),
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX push_gateway_challenges_expiry ON push_gateway_challenges (expires_at);

CREATE TABLE push_gateway_installations (
    id UUID PRIMARY KEY,
    app_attest_key_id BYTEA NOT NULL CHECK (octet_length(app_attest_key_id) BETWEEN 1 AND 128),
    app_attest_public_key BYTEA NOT NULL CHECK (octet_length(app_attest_public_key) BETWEEN 33 AND 256),
    assertion_counter BIGINT NOT NULL CHECK (assertion_counter BETWEEN 0 AND 4294967295),
    app_profile TEXT NOT NULL CHECK (app_profile = 'kuzz-ios-dogfood'),
    token_ciphertext BYTEA NOT NULL CHECK (octet_length(token_ciphertext) BETWEEN 1 AND 2048),
    token_fingerprint BYTEA NOT NULL CHECK (length(token_fingerprint) = 32),
    endpoint_epoch BIGINT NOT NULL CHECK (endpoint_epoch > 0),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX push_gateway_installations_active_app_attest_key
    ON push_gateway_installations (app_attest_key_id) WHERE revoked_at IS NULL;
CREATE UNIQUE INDEX push_gateway_installations_active_profile_token
    ON push_gateway_installations (app_profile, token_fingerprint) WHERE revoked_at IS NULL;
CREATE INDEX push_gateway_installations_expiry ON push_gateway_installations (expires_at) WHERE revoked_at IS NULL;

CREATE TABLE push_gateway_delegations (
    id UUID PRIMARY KEY,
    installation_id UUID NOT NULL REFERENCES push_gateway_installations(id),
    relay_pubkey BYTEA NOT NULL CHECK (length(relay_pubkey) = 32),
    endpoint_epoch BIGINT NOT NULL CHECK (endpoint_epoch > 0),
    generation BIGINT NOT NULL CHECK (generation > 0),
    not_before TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (installation_id, relay_pubkey),
    CHECK (not_before < expires_at)
);
CREATE INDEX push_gateway_delegations_expiry ON push_gateway_delegations (expires_at) WHERE revoked_at IS NULL;

CREATE TABLE push_gateway_endpoint_quotas (
    token_fingerprint BYTEA PRIMARY KEY CHECK (length(token_fingerprint) = 32),
    window_started_at TIMESTAMPTZ NOT NULL,
    admitted BIGINT NOT NULL CHECK (admitted >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX push_gateway_endpoint_quotas_updated ON push_gateway_endpoint_quotas (updated_at);

CREATE TABLE push_gateway_delivery_auth_replays (
    relay_pubkey BYTEA NOT NULL CHECK (length(relay_pubkey) = 32),
    auth_event_id BYTEA NOT NULL CHECK (length(auth_event_id) = 32),
    expires_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (relay_pubkey, auth_event_id)
);
CREATE INDEX push_gateway_delivery_auth_replays_expiry ON push_gateway_delivery_auth_replays (expires_at);

CREATE TABLE push_gateway_delivery_request_replays (
    relay_pubkey BYTEA NOT NULL CHECK (length(relay_pubkey) = 32),
    request_id UUID NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (relay_pubkey, request_id)
);
CREATE INDEX push_gateway_delivery_request_replays_expiry ON push_gateway_delivery_request_replays (expires_at);

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('push_gateway_challenges', 'public gateway one-time challenges span relay communities'),
    ('push_gateway_installations', 'public gateway installation authority spans relay communities'),
    ('push_gateway_delegations', 'public gateway relay delegations span relay communities'),
    ('push_gateway_endpoint_quotas', 'public gateway endpoint abuse ceilings span relay communities'),
    ('push_gateway_delivery_auth_replays', 'public gateway signed-event replay admission spans relay communities'),
    ('push_gateway_delivery_request_replays', 'public gateway stable request-id admission spans relay communities');

-- ── Replica heartbeat (read-replica freshness fence) ─────────────────────────
-- Portable read-side freshness observation for the replica fence (see
-- crates/kuzz-db/src/replica_fence.rs and migrations/0026). Exactly one row;
-- the single-row token UPDATE is the serialization point that makes tokens
-- globally commit-ordered across relay pods. `epoch` detects token resets
-- (restore/re-seed) so a stale retained token can never masquerade as fresh
-- coverage. Deployment-global by design: describes replication topology,
-- never tenant data.

CREATE TABLE replica_heartbeat (
    id    smallint PRIMARY KEY CHECK (id = 1),
    epoch uuid     NOT NULL DEFAULT gen_random_uuid(),
    token bigint   NOT NULL DEFAULT 0
) WITH (
    vacuum_truncate = false
);

INSERT INTO replica_heartbeat (id) VALUES (1);

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('replica_heartbeat', 'single-row replication freshness token; describes deployment topology, never tenant data');

-- ── Whole-community deletion control plane (migration 0029) ─────────────────
CREATE TABLE community_deletion_requests (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    community_id UUID NOT NULL REFERENCES communities(id),
    community_host TEXT NOT NULL,
    stage TEXT NOT NULL DEFAULT 'submitted' CHECK (stage IN (
        'submitted', 'inventoried', 'approved', 'fenced', 'drained',
        'bindings_removed', 'postgres_purged', 'cache_purged',
        'logically_verified', 'retention_pending', 'aborted'
    )),
    requested_by TEXT NOT NULL,
    request_origin TEXT NOT NULL DEFAULT 'operator'
        CHECK (request_origin IN ('operator', 'owner')),
    owner_pubkey TEXT,
    mediating_operator_pubkey TEXT,
    acknowledgement_version INTEGER,
    reason TEXT,
    schema_manifest JSONB,
    storage_manifest JSONB,
    destructive_storage_manifest JSONB,
    destructive_storage_frozen_at TIMESTAMPTZ,
    inventory_manifest JSONB,
    inventory_digest BYTEA CHECK (inventory_digest IS NULL OR length(inventory_digest) = 32),
    inventory_frozen_at TIMESTAMPTZ,
    fence_generation BIGINT CHECK (fence_generation IS NULL OR fence_generation > 0),
    lease_owner TEXT,
    lease_generation BIGINT NOT NULL DEFAULT 0 CHECK (lease_generation >= 0),
    lease_until TIMESTAMPTZ,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    retry_count INTEGER NOT NULL DEFAULT 0 CHECK (retry_count >= 0),
    retry_stage TEXT CHECK (retry_stage IS NULL OR retry_stage IN (
        'submitted', 'approved', 'fenced', 'drained', 'bindings_removed',
        'postgres_purged', 'cache_purged', 'logically_verified'
    )),
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_error TEXT,
    last_error_at TIMESTAMPTZ,
    blocked_at TIMESTAMPTZ,
    blocked_reason TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    pre_quiesce_archived_at TIMESTAMPTZ,
    quiescing_started_at TIMESTAMPTZ,
    aborted_by TEXT,
    abort_reason TEXT,
    aborted_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    CHECK ((blocked_at IS NULL) = (blocked_reason IS NULL)),
    CHECK ((stage = 'aborted') = (aborted_at IS NOT NULL)),
    CHECK ((aborted_at IS NULL) = (aborted_by IS NULL)),
    CHECK ((aborted_at IS NULL) = (abort_reason IS NULL)),
    CHECK ((inventory_frozen_at IS NULL) = (inventory_digest IS NULL)),
    CONSTRAINT community_deletion_owner_provenance CHECK (
        (request_origin = 'operator'
            AND owner_pubkey IS NULL
            AND mediating_operator_pubkey IS NULL
            AND acknowledgement_version IS NULL)
        OR
        (request_origin = 'owner'
            AND NOT (owner_pubkey IS NULL)
            AND NOT (mediating_operator_pubkey IS NULL)
            AND NOT (acknowledgement_version IS NULL)
            AND owner_pubkey ~ '^[0-9a-f]{64}$'
            AND mediating_operator_pubkey ~ '^[0-9a-f]{64}$'
            AND acknowledgement_version BETWEEN 1 AND 32767
            AND requested_by = owner_pubkey)
    ),
    UNIQUE (id, community_id, inventory_digest)
);
CREATE UNIQUE INDEX community_deletion_requests_active_community
    ON community_deletion_requests (community_id)
    WHERE stage <> 'aborted';
CREATE INDEX community_deletion_requests_runnable
    ON community_deletion_requests (next_attempt_at, created_at)
    WHERE blocked_at IS NULL
      AND stage IN ('approved', 'fenced', 'drained', 'bindings_removed',
                    'postgres_purged', 'cache_purged', 'logically_verified');
CREATE INDEX community_deletion_requests_lease
    ON community_deletion_requests (lease_until) WHERE lease_owner IS NOT NULL;
CREATE INDEX community_deletion_requests_owner_preparable
    ON community_deletion_requests (next_attempt_at, created_at)
    WHERE request_origin = 'owner'
      AND stage = 'submitted'
      AND blocked_at IS NULL;
CREATE INDEX community_deletion_requests_owner_quota_reservations
    ON community_deletion_requests (owner_pubkey)
    INCLUDE (community_id, completed_at)
    WHERE request_origin = 'owner'
      AND stage <> 'aborted';

CREATE TABLE community_deletion_approvals (
    request_id UUID PRIMARY KEY,
    community_id UUID NOT NULL,
    inventory_digest BYTEA NOT NULL CHECK (length(inventory_digest) = 32),
    approved_by TEXT NOT NULL,
    approval_origin TEXT NOT NULL DEFAULT 'operator'
        CHECK (approval_origin IN ('operator', 'owner_automatic')),
    note TEXT,
    approved_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (request_id, community_id, inventory_digest)
        REFERENCES community_deletion_requests(id, community_id, inventory_digest)
        ON DELETE RESTRICT
);

CREATE OR REPLACE FUNCTION prevent_community_deletion_request_retargeting()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.community_id IS DISTINCT FROM OLD.community_id
        OR NEW.community_host IS DISTINCT FROM OLD.community_host
    THEN
        RAISE EXCEPTION 'community deletion target identity is immutable'
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    IF NEW.request_origin IS DISTINCT FROM OLD.request_origin
        OR NEW.owner_pubkey IS DISTINCT FROM OLD.owner_pubkey
        OR NEW.mediating_operator_pubkey IS DISTINCT FROM OLD.mediating_operator_pubkey
        OR NEW.acknowledgement_version IS DISTINCT FROM OLD.acknowledgement_version
    THEN
        RAISE EXCEPTION 'community deletion request provenance is immutable'
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    IF OLD.inventory_frozen_at IS NOT NULL AND (
        NEW.schema_manifest IS DISTINCT FROM OLD.schema_manifest
        OR NEW.storage_manifest IS DISTINCT FROM OLD.storage_manifest
        OR NEW.inventory_manifest IS DISTINCT FROM OLD.inventory_manifest
        OR NEW.inventory_digest IS DISTINCT FROM OLD.inventory_digest
        OR NEW.inventory_frozen_at IS DISTINCT FROM OLD.inventory_frozen_at
    ) THEN
        RAISE EXCEPTION 'frozen community deletion inventory is immutable'
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    IF OLD.destructive_storage_frozen_at IS NOT NULL AND (
        NEW.destructive_storage_manifest IS DISTINCT FROM OLD.destructive_storage_manifest
        OR NEW.destructive_storage_frozen_at IS DISTINCT FROM OLD.destructive_storage_frozen_at
    ) THEN
        RAISE EXCEPTION 'frozen destructive storage manifest is immutable'
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER community_deletion_request_retargeting_guard
BEFORE UPDATE ON community_deletion_requests
FOR EACH ROW
EXECUTE FUNCTION prevent_community_deletion_request_retargeting();

CREATE FUNCTION prevent_community_deletion_approval_removal()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'community deletion approval evidence is immutable'
        USING ERRCODE = 'integrity_constraint_violation';
END;
$$;

CREATE TRIGGER community_deletion_approval_removal_guard
BEFORE UPDATE OR DELETE ON community_deletion_approvals
FOR EACH ROW
EXECUTE FUNCTION prevent_community_deletion_approval_removal();

CREATE TABLE community_deletion_checkpoints (
    request_id UUID NOT NULL REFERENCES community_deletion_requests(id) ON DELETE RESTRICT,
    sequence BIGINT GENERATED ALWAYS AS IDENTITY,
    stage TEXT NOT NULL,
    unit_key TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('started', 'completed', 'failed')),
    lease_generation BIGINT NOT NULL CHECK (lease_generation > 0),
    attempts INTEGER NOT NULL DEFAULT 1 CHECK (attempts > 0),
    detail JSONB NOT NULL DEFAULT '{}'::jsonb,
    error TEXT,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ,
    PRIMARY KEY (request_id, sequence),
    UNIQUE (request_id, stage, unit_key),
    CHECK ((status = 'completed') = (completed_at IS NOT NULL)),
    CHECK ((status = 'failed') = (error IS NOT NULL))
);

-- Frozen destructive key list, chunked out of the request row so a large
-- tenant (100k-1M objects) never materializes as one multi-hundred-MB JSONB
-- value. Rows are written once in the fenced stage, stamped `deleted_at` as
-- the executor confirms each chunk removed, and dropped at logical
-- verification. The request row keeps only per-prefix count/bytes/digest
-- summaries; the chunk stream must hash to those frozen digests.
CREATE TABLE community_deletion_manifest_keys (
    request_id UUID NOT NULL REFERENCES community_deletion_requests(id) ON DELETE CASCADE,
    chunk_no BIGINT NOT NULL CHECK (chunk_no >= 0),
    prefix TEXT NOT NULL,
    keys JSONB NOT NULL,
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (request_id, chunk_no)
);

-- Chunk content is immutable once written; the only permitted update is the
-- one-way deleted_at stamp. New chunks are permitted only while the request is
-- fenced and its destructive manifest remains unfrozen. Removal is permitted
-- only while the destructive manifest has not yet frozen (a retried partial
-- freeze rewrites its chunks) or once the request has passed logical
-- verification (terminal cleanup).
CREATE FUNCTION protect_community_deletion_manifest_keys()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    frozen_at TIMESTAMPTZ;
    request_stage TEXT;
BEGIN
    IF TG_OP = 'UPDATE' THEN
        IF NEW.request_id IS DISTINCT FROM OLD.request_id
            OR NEW.chunk_no IS DISTINCT FROM OLD.chunk_no
            OR NEW.prefix IS DISTINCT FROM OLD.prefix
            OR NEW.keys IS DISTINCT FROM OLD.keys
            OR OLD.deleted_at IS NOT NULL
        THEN
            RAISE EXCEPTION 'community deletion manifest key chunks are immutable'
                USING ERRCODE = 'integrity_constraint_violation';
        END IF;
        RETURN NEW;
    END IF;
    SELECT destructive_storage_frozen_at, stage
      INTO frozen_at, request_stage
      FROM community_deletion_requests
     WHERE id = CASE WHEN TG_OP = 'INSERT' THEN NEW.request_id ELSE OLD.request_id END
     FOR UPDATE;
    IF TG_OP = 'INSERT' THEN
        IF FOUND AND frozen_at IS NULL AND request_stage = 'fenced' THEN
            RETURN NEW;
        END IF;
        RAISE EXCEPTION 'community deletion manifest key chunks require an unfrozen fenced request'
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    IF NOT FOUND
        OR frozen_at IS NULL
        OR request_stage IN ('logically_verified', 'retention_pending')
    THEN
        RETURN OLD;
    END IF;
    RAISE EXCEPTION 'community deletion manifest key chunks cannot be removed mid-execution'
        USING ERRCODE = 'integrity_constraint_violation';
END;
$$;

CREATE TRIGGER community_deletion_manifest_keys_guard
BEFORE INSERT OR UPDATE OR DELETE ON community_deletion_manifest_keys
FOR EACH ROW
EXECUTE FUNCTION protect_community_deletion_manifest_keys();

-- Fleet-wide object-store taxonomy sweep evidence. This is an independent
-- observability record: community deletion inventories only the target's owned
-- prefixes and does not gate submission or execution on sweep state.
CREATE TABLE storage_taxonomy_sweeps (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    started_at TIMESTAMPTZ NOT NULL,
    completed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    listed_objects BIGINT NOT NULL CHECK (listed_objects >= 0),
    unknown_object_count BIGINT NOT NULL CHECK (unknown_object_count >= 0),
    unknown_key_sample JSONB NOT NULL DEFAULT '[]'::jsonb,
    object_cap BIGINT NOT NULL CHECK (object_cap > 0),
    CHECK (completed_at >= started_at)
);
CREATE INDEX storage_taxonomy_sweeps_latest
    ON storage_taxonomy_sweeps (completed_at DESC);

CREATE TABLE community_serving_write_leases (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    community_id UUID NOT NULL REFERENCES communities(id),
    operation TEXT NOT NULL,
    owner TEXT NOT NULL,
    generation BIGINT NOT NULL DEFAULT 1 CHECK (generation > 0),
    -- Community fence generation observed when this lease was acquired.
    fence_generation BIGINT NOT NULL CHECK (fence_generation >= 0),
    lease_until TIMESTAMPTZ NOT NULL,
    heartbeat_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX community_serving_write_leases_active
    ON community_serving_write_leases (community_id, lease_until);

CREATE TABLE community_deletion_executor_heartbeats (
    executor_id TEXT PRIMARY KEY,
    mode TEXT NOT NULL CHECK (mode IN ('run', 'drain', 'worker')),
    request_id UUID REFERENCES community_deletion_requests(id) ON DELETE SET NULL,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    heartbeat_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    draining BOOLEAN NOT NULL DEFAULT false,
    stopped_at TIMESTAMPTZ
);
INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('community_deletion_requests', 'deployment deletion lifecycle and frozen inventory'),
    ('community_deletion_approvals', 'deployment operator destructive approvals'),
    ('community_deletion_checkpoints', 'deployment deletion executor checkpoints and failures'),
    ('community_deletion_manifest_keys', 'deployment deletion frozen destructive key chunks'),
    ('storage_taxonomy_sweeps', 'deployment object-store taxonomy sweep evidence'),
    ('community_serving_write_leases', 'deployment serving side-effect leases drained by deletion'),
    ('community_deletion_executor_heartbeats', 'deployment deletion worker liveness');

CREATE FUNCTION community_deletion_lock_key(target UUID) RETURNS BIGINT
LANGUAGE SQL IMMUTABLE STRICT PARALLEL SAFE AS $$
    SELECT hashtextextended('kuzz-community-deletion:' || target::text, 0)
$$;
-- Keep the deletion control plane writable while its target tenant is fenced.
-- This predicate is the single SQL source of truth used by attachment and live
-- catalog validation.
CREATE FUNCTION community_write_fence_excluded_table(target NAME) RETURNS BOOLEAN
LANGUAGE SQL IMMUTABLE STRICT PARALLEL SAFE AS $$
    SELECT target::TEXT = ANY (ARRAY[
        'community_deletion_requests', 'community_deletion_approvals',
        'community_deletion_checkpoints', 'community_serving_write_leases',
        'community_deletion_executor_heartbeats', 'product_feedback',
        'rate_limit_violations', 'operator_listener_outbox'
    ]::TEXT[])
$$;

-- Fleet-wide writers filter candidates through this VOLATILE predicate in
-- the mutating statement so fenced tenants are skipped before row triggers run.
CREATE FUNCTION community_write_allowed(target UUID) RETURNS BOOLEAN
LANGUAGE plpgsql VOLATILE AS $$
DECLARE
    lifecycle TEXT;
BEGIN
    IF current_setting('transaction_isolation') <> 'read committed' THEN
        RAISE EXCEPTION 'community writes require READ COMMITTED isolation'
            USING ERRCODE = 'invalid_transaction_state';
    END IF;

    IF target IS NULL THEN
        RETURN true;
    END IF;

    PERFORM pg_advisory_xact_lock_shared(community_deletion_lock_key(target));
    SELECT deletion_state
      INTO lifecycle
      FROM communities
     WHERE id = target;
    RETURN FOUND AND lifecycle = 'active';
END
$$;

CREATE FUNCTION assert_community_write_allowed(target UUID) RETURNS VOID
LANGUAGE plpgsql AS $$
DECLARE
    lifecycle TEXT;
    generation BIGINT;
    executor_community TEXT;
    executor_generation TEXT;
    serving_community TEXT;
    serving_lease_id TEXT;
    serving_owner TEXT;
    serving_generation TEXT;
    serving_fence_generation TEXT;
    serving_lease_valid BOOLEAN := false;
BEGIN
    -- The fence proof requires a fresh statement snapshot after lock grant;
    -- pinned RR/Serializable snapshots can retain pre-fence authorization.
    IF current_setting('transaction_isolation') <> 'read committed' THEN
        RAISE EXCEPTION 'community writes require READ COMMITTED isolation'
            USING ERRCODE = 'invalid_transaction_state';
    END IF;

    -- Nullable operator-attribution rows without a tenant are unrelated.
    IF target IS NULL THEN
        RETURN;
    END IF;

    PERFORM pg_advisory_xact_lock_shared(community_deletion_lock_key(target));
    SELECT deletion_state, deletion_fence_generation
      INTO lifecycle, generation
      FROM communities
     WHERE id = target;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'community write rejected: community % is missing', target
            USING ERRCODE = 'object_not_in_prerequisite_state';
    END IF;

    -- Authorization is evaluated independently for every community checked.
    executor_community := current_setting('kuzz.deletion_executor_community', true);
    executor_generation := current_setting('kuzz.deletion_fence_generation', true);
    IF executor_community = target::TEXT
       AND executor_generation ~ '^[0-9]+$'
       AND executor_generation::BIGINT = generation THEN
        RETURN;
    END IF;

    -- A serving mutation admitted before quiescing may finish only while its
    -- exact durable lease remains current and bound to this fence generation.
    serving_community := current_setting('kuzz.serving_write_community', true);
    serving_lease_id := current_setting('kuzz.serving_write_lease_id', true);
    serving_owner := current_setting('kuzz.serving_write_owner', true);
    serving_generation := current_setting('kuzz.serving_write_generation', true);
    serving_fence_generation := current_setting('kuzz.serving_write_fence_generation', true);
    IF lifecycle IN ('active', 'quiescing')
       AND serving_community = target::TEXT
       AND serving_lease_id ~ '^[0-9a-fA-F-]{36}$'
       AND serving_generation ~ '^[0-9]+$'
       AND serving_fence_generation ~ '^[0-9]+$'
       AND serving_fence_generation::BIGINT = generation THEN
        SELECT EXISTS(
            SELECT 1 FROM community_serving_write_leases lease
             WHERE lease.id = serving_lease_id::UUID
               AND lease.community_id = target
               AND lease.owner = serving_owner
               AND lease.generation = serving_generation::BIGINT
               AND lease.fence_generation = serving_fence_generation::BIGINT
               AND lease.lease_until >= now()
        ) INTO serving_lease_valid;
        IF serving_lease_valid THEN
            RETURN;
        END IF;
    END IF;

    IF lifecycle <> 'active' THEN
        RAISE EXCEPTION 'community write fenced: community % generation %', target, generation
            USING ERRCODE = 'object_not_in_prerequisite_state';
    END IF;
END
$$;

CREATE FUNCTION enforce_community_write_fence() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        PERFORM assert_community_write_allowed(NEW.community_id);
    ELSIF TG_OP = 'DELETE' THEN
        PERFORM assert_community_write_allowed(OLD.community_id);
    ELSIF OLD.community_id IS NOT DISTINCT FROM NEW.community_id THEN
        PERFORM assert_community_write_allowed(OLD.community_id);
    ELSIF OLD.community_id IS NULL THEN
        PERFORM assert_community_write_allowed(NEW.community_id);
    ELSIF NEW.community_id IS NULL THEN
        PERFORM assert_community_write_allowed(OLD.community_id);
    ELSIF OLD.community_id < NEW.community_id THEN
        PERFORM assert_community_write_allowed(OLD.community_id);
        PERFORM assert_community_write_allowed(NEW.community_id);
    ELSE
        PERFORM assert_community_write_allowed(NEW.community_id);
        PERFORM assert_community_write_allowed(OLD.community_id);
    END IF;

    RETURN CASE WHEN TG_OP = 'DELETE' THEN OLD ELSE NEW END;
END
$$;

CREATE FUNCTION enforce_community_tombstone() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
DECLARE
    executor_community TEXT := current_setting('kuzz.deletion_executor_community', true);
    executor_generation TEXT := current_setting('kuzz.deletion_fence_generation', true);
    expected_generation BIGINT;
BEGIN
    IF TG_OP = 'DELETE' THEN
        IF OLD.deletion_state <> 'active' OR OLD.deleted_at IS NOT NULL THEN
            RAISE EXCEPTION 'community tombstones are permanent'
                USING ERRCODE = 'object_not_in_prerequisite_state';
        END IF;
        RETURN OLD;
    END IF;
    expected_generation := CASE WHEN NEW.deletion_fence_generation > OLD.deletion_fence_generation
        THEN NEW.deletion_fence_generation ELSE OLD.deletion_fence_generation END;
    IF executor_community = OLD.id::text AND executor_generation ~ '^[0-9]+$'
       AND executor_generation::BIGINT = expected_generation THEN RETURN NEW; END IF;
    IF OLD.deletion_state <> 'active' OR NEW.deletion_state <> OLD.deletion_state
       OR NEW.deletion_fence_generation <> OLD.deletion_fence_generation
       OR NEW.deleted_at IS DISTINCT FROM OLD.deleted_at THEN
        RAISE EXCEPTION 'community tombstone mutation rejected: community % generation %',
            OLD.id, OLD.deletion_fence_generation
            USING ERRCODE = 'object_not_in_prerequisite_state';
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER communities_deletion_tombstone BEFORE UPDATE OR DELETE ON communities
FOR EACH ROW EXECUTE FUNCTION enforce_community_tombstone();
-- Attach the universal fence to one community-scoped relation. Future
-- migrations must invoke this helper explicitly after CREATE/ALTER introduces
-- community_id; the migration lint enforces that contract.
CREATE FUNCTION attach_community_write_fence(target REGCLASS) RETURNS VOID
LANGUAGE plpgsql AS $$
DECLARE
    relation_name NAME;
BEGIN
    SELECT c.relname
      INTO relation_name
      FROM pg_class c
      JOIN pg_namespace n ON n.oid = c.relnamespace
     WHERE c.oid = target
       AND n.nspname = current_schema()
       AND c.relkind IN ('r', 'p')
       AND NOT c.relispartition;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'community write fence target % is not a table in the current schema', target
            USING ERRCODE = 'wrong_object_type';
    END IF;
    IF community_write_fence_excluded_table(relation_name) THEN
        RETURN;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_attribute
         WHERE attrelid = target AND attname = 'community_id' AND NOT attisdropped
    ) THEN
        RAISE EXCEPTION 'community write fence target % has no community_id', target
            USING ERRCODE = 'undefined_column';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_trigger
         WHERE tgrelid = target
           AND tgname = 'community_write_fence_' || relation_name
           AND NOT tgisinternal
    ) THEN
        EXECUTE format(
            'CREATE TRIGGER %I BEFORE INSERT OR UPDATE OR DELETE ON %s '
            'FOR EACH ROW EXECUTE FUNCTION enforce_community_write_fence()',
            'community_write_fence_' || relation_name,
            target
        );
    END IF;
END
$$;

-- Attach the universal fence to every existing table carrying community_id,
-- including deployment-private sidecars whose community_id is provenance.
DO $$
DECLARE
    target REGCLASS;
BEGIN
    FOR target IN
        SELECT c.oid::REGCLASS
          FROM pg_class c
          JOIN pg_namespace n ON n.oid = c.relnamespace
          JOIN pg_attribute a ON a.attrelid = c.oid
         WHERE n.nspname = current_schema()
           AND c.relkind IN ('r', 'p')
           AND NOT c.relispartition
           AND a.attname = 'community_id'
           AND NOT a.attisdropped
           AND NOT community_write_fence_excluded_table(c.relname)
         ORDER BY c.oid::REGCLASS::TEXT
    LOOP
        PERFORM attach_community_write_fence(target);
    END LOOP;
END
$$;

-- Desired-state schema application does not replay migration history, so keep
-- these explicit calls as first-class catalog declarations. They also make the
-- fence contract visible to migration linting instead of hiding it only in the
-- dynamic bootstrap loop above.
SELECT attach_community_write_fence('api_tokens');
SELECT attach_community_write_fence('archived_identities');
SELECT attach_community_write_fence('audit_log');
SELECT attach_community_write_fence('channel_members');
SELECT attach_community_write_fence('channels');
SELECT attach_community_write_fence('community_bans');
SELECT attach_community_write_fence('delivery_log');
SELECT attach_community_write_fence('event_mentions');
SELECT attach_community_write_fence('events');
SELECT attach_community_write_fence('git_repo_names');
SELECT attach_community_write_fence('group_add_settings');
SELECT attach_community_write_fence('group_invitations');
SELECT attach_community_write_fence('group_invite_links');
SELECT attach_community_write_fence('join_policy_acceptances');
SELECT attach_community_write_fence('media_references');
SELECT attach_community_write_fence('media_uploads');
SELECT attach_community_write_fence('moderation_actions');
SELECT attach_community_write_fence('moderation_reports');
SELECT attach_community_write_fence('parameterized_event_watermarks');
SELECT attach_community_write_fence('pubkey_allowlist');
SELECT attach_community_write_fence('push_leases');
SELECT attach_community_write_fence('push_match_queue');
SELECT attach_community_write_fence('push_wake_outbox');
SELECT attach_community_write_fence('reactions');
SELECT attach_community_write_fence('relay_invites');
SELECT attach_community_write_fence('relay_members');
SELECT attach_community_write_fence('scheduled_workflow_fires');
SELECT attach_community_write_fence('subscriptions');
SELECT attach_community_write_fence('thread_follows');
SELECT attach_community_write_fence('thread_metadata');
SELECT attach_community_write_fence('thread_state');
SELECT attach_community_write_fence('user_block_periods');
SELECT attach_community_write_fence('user_blocks');
SELECT attach_community_write_fence('users');
SELECT attach_community_write_fence('workflow_approvals');
SELECT attach_community_write_fence('workflow_runs');
SELECT attach_community_write_fence('workflows');

-- ── Relay operator/moderator roster ──────────────────────────────────────────
-- Deployment-level principals staffed via the admin API. Config-backed operators
-- (RELAY_OPERATOR_PUBKEYS, RELAY_OWNER_PUBKEY owner-fallback) are NOT seeded here;
-- they are authoritative in config and outrank any DB row.

CREATE TABLE relay_operators (
    pubkey      BYTEA NOT NULL PRIMARY KEY CHECK (length(pubkey) = 32),
    role        TEXT NOT NULL CHECK (role IN ('operator', 'moderator')),
    added_by    BYTEA NOT NULL CHECK (length(added_by) = 32),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('relay_operators', 'deployment-global operator/moderator roster; no community_id intentionally');

-- ── Centralized identity (migrations 0056/0058) ──────────────────────────────
-- Server accounts, devices, sessions, bots and opaque access-token hashes.
-- Deployment-global (no community_id). Principal ids are valid x-only
-- secp256k1 public keys; the store layer is the only writer. The relay
-- principal row is created by relay startup code, not seeded here.

CREATE TABLE principals (
    identity_mode TEXT NOT NULL DEFAULT 'token' CONSTRAINT principals_identity_mode CHECK (identity_mode IN ('token', 'key_backup')),
    id            BYTEA PRIMARY KEY CHECK (length(id) = 32),
    kind          TEXT NOT NULL CHECK (kind IN ('user', 'bot', 'relay')),
    display_name  TEXT NOT NULL DEFAULT '',
    avatar_url    TEXT,
    username      TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    disabled_at   TIMESTAMPTZ,
    purge_after   TIMESTAMPTZ,
    discoverable  BOOLEAN NOT NULL DEFAULT true,
    -- Language the user picked in the app (migration 0087); NULL = English.
    locale        TEXT CONSTRAINT principals_locale CHECK (locale ~ '^[a-z]{2,3}(-[A-Za-z0-9]{2,8}){0,2}$')
);
CREATE UNIQUE INDEX principals_username_lower
    ON principals (lower(username)) WHERE username IS NOT NULL;
-- Exactly one relay principal per deployment; concurrent first boots converge.
CREATE UNIQUE INDEX principals_single_relay
    ON principals (kind) WHERE kind = 'relay';

-- External identities. Adding a provider (Apple, ...) is a CHECK extension.
CREATE TABLE identities (
    provider      TEXT NOT NULL CHECK (provider IN ('google')),
    subject       TEXT NOT NULL,
    principal_id  BYTEA NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
    email         TEXT,
    linked_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_login_at TIMESTAMPTZ,
    PRIMARY KEY (provider, subject)
);
CREATE INDEX identities_principal ON identities (principal_id);
CREATE INDEX identities_email_lower ON identities (lower(email)) WHERE email IS NOT NULL;

CREATE TABLE devices (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    principal_id  BYTEA NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    platform      TEXT NOT NULL CHECK (platform IN ('desktop', 'mobile', 'web', 'cli', 'external')),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at    TIMESTAMPTZ,
    -- Stable per-install id; re-login with it reuses the row (migration 0063).
    install_id    TEXT,
    -- Device key (P-256, SEC1 compressed) proving use from this device (0078).
    device_pubkey BYTEA CHECK (device_pubkey IS NULL OR octet_length(device_pubkey) = 33),
    device_key_kind TEXT CHECK (device_key_kind IS NULL OR device_key_kind IN ('hardware', 'software')),
    CONSTRAINT devices_key_kind_with_key CHECK ((device_pubkey IS NULL) = (device_key_kind IS NULL)),
    -- `login` (signed in with the identity provider) or `external` (an
    -- external agent's computer paired by the owner, 0090).
    kind          TEXT NOT NULL DEFAULT 'login' CHECK (kind IN ('login', 'external')),
    CONSTRAINT devices_external_shape CHECK ((kind = 'external') = (platform = 'external')
           AND (kind = 'login' OR (device_pubkey IS NOT NULL AND install_id IS NULL)))
);
CREATE INDEX devices_principal ON devices (principal_id);
CREATE UNIQUE INDEX devices_principal_install
    ON devices (principal_id, install_id)
    WHERE install_id IS NOT NULL;
CREATE UNIQUE INDEX devices_principal_key
    ON devices (principal_id, kind, device_pubkey)
    WHERE device_pubkey IS NOT NULL AND revoked_at IS NULL;

CREATE TABLE sessions (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    device_id         UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_refreshed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at        TIMESTAMPTZ,
    revoked_reason    TEXT
);
CREATE INDEX sessions_device ON sessions (device_id);

-- Refresh-token rotation history: one row per issued refresh token. A consumed
-- row keeps used_at so a replay is detectable (refresh reuse => session revoke).
CREATE TABLE refresh_tokens (
    token_hash    BYTEA PRIMARY KEY CHECK (length(token_hash) = 32),
    session_id    UUID NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    generation    INT  NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at    TIMESTAMPTZ NOT NULL,
    used_at       TIMESTAMPTZ
);
CREATE UNIQUE INDEX refresh_tokens_live
    ON refresh_tokens (session_id) WHERE used_at IS NULL;
CREATE INDEX refresh_tokens_session ON refresh_tokens (session_id);

-- owner_principal_id intentionally has no ON DELETE action: purging an owner
-- without first deleting its bots fails instead of orphaning them.
CREATE TABLE bots (
    id                 BYTEA PRIMARY KEY REFERENCES principals(id) ON DELETE CASCADE,
    owner_principal_id BYTEA NOT NULL REFERENCES principals(id),
    host_device_id     UUID REFERENCES devices(id) ON DELETE SET NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at         TIMESTAMPTZ
);
CREATE INDEX bots_owner ON bots (owner_principal_id) WHERE deleted_at IS NULL;
CREATE INDEX bots_host_device ON bots (host_device_id) WHERE deleted_at IS NULL;

CREATE TABLE access_tokens (
    token_hash       BYTEA PRIMARY KEY CHECK (length(token_hash) = 32),
    principal_id     BYTEA NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
    kind             TEXT NOT NULL CHECK (kind IN ('user', 'bot', 'bot_headless')),
    session_id       UUID REFERENCES sessions(id) ON DELETE CASCADE,
    bot_id           BYTEA REFERENCES bots(id) ON DELETE CASCADE,
    issued_by_device UUID REFERENCES devices(id) ON DELETE CASCADE,
    expires_at       TIMESTAMPTZ,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at     TIMESTAMPTZ,
    superseded_at    TIMESTAMPTZ,
    revoked_at       TIMESTAMPTZ,
    revoked_reason   TEXT,
    -- Named so scripts/reconcile-schema-after-pgschema.sql can restore it:
    -- pgschema drops multi-column CHECK constraints.
    CONSTRAINT access_tokens_kind_shape CHECK (
        (kind = 'user' AND session_id IS NOT NULL AND bot_id IS NULL)
        OR (kind <> 'user' AND bot_id IS NOT NULL AND session_id IS NULL))
);
CREATE INDEX access_tokens_principal ON access_tokens (principal_id) WHERE revoked_at IS NULL;
CREATE INDEX access_tokens_session ON access_tokens (session_id) WHERE revoked_at IS NULL;
CREATE INDEX access_tokens_bot ON access_tokens (bot_id) WHERE revoked_at IS NULL;

-- Custodial recovery is independent of Nostr messaging identity and transport.
CREATE TABLE account_key_backups (
    account_id BYTEA PRIMARY KEY REFERENCES principals(id),
    pubkey BYTEA NOT NULL UNIQUE CHECK (length(pubkey) = 32),
    version SMALLINT NOT NULL CHECK (version IN (1, 2)),
    key_id TEXT NOT NULL CHECK (key_id ~ '^[A-Za-z0-9_-]{1,64}$'),
    nonce BYTEA NOT NULL CHECK (length(nonce) = 12),
    ciphertext BYTEA NOT NULL CHECK (length(ciphertext) = 48),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('principals', 'deployment-global server accounts (users, bots, relay); no community_id intentionally'),
    ('identities', 'deployment-global external OIDC identities linked to principals'),
    ('devices', 'deployment-global login devices of a principal'),
    ('sessions', 'deployment-global login sessions per device'),
    ('refresh_tokens', 'deployment-global refresh-token rotation history'),
    ('bots', 'deployment-global bot registrations owned by a user principal'),
    ('access_tokens', 'deployment-global opaque access-token hashes'),
    ('account_key_backups', 'deployment-global immutable Google account to Nostr key backup binding');

-- Which agent key may be used on which device (migration 0078).
CREATE TABLE key_registry (
    pubkey         BYTEA PRIMARY KEY CHECK (length(pubkey) = 32),
    principal_id   BYTEA NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
    role           TEXT NOT NULL CHECK (role IN ('agent', 'external')),
    host_device_id UUID REFERENCES devices(id) ON DELETE SET NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at     TIMESTAMPTZ
);
CREATE INDEX key_registry_principal ON key_registry (principal_id);
CREATE INDEX key_registry_host ON key_registry (host_device_id) WHERE host_device_id IS NOT NULL;

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('key_registry', 'deployment-global agent key to owner account and host device binding');

-- Encrypted Kuzz agent key copies (migration 0080).
CREATE TABLE agent_key_backups (
    agent_pubkey   BYTEA PRIMARY KEY REFERENCES key_registry(pubkey) ON DELETE CASCADE,
    owner_id       BYTEA NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
    host_device_id UUID REFERENCES devices(id) ON DELETE SET NULL,
    version        SMALLINT NOT NULL CHECK (version = 1),
    key_id         TEXT NOT NULL CHECK (key_id ~ '^[A-Za-z0-9_-]{1,64}$'),
    nonce          BYTEA NOT NULL CHECK (length(nonce) = 12),
    ciphertext     BYTEA NOT NULL CHECK (length(ciphertext) = 48),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX agent_key_backups_owner_host ON agent_key_backups (owner_id, host_device_id);

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('agent_key_backups', 'deployment-global encrypted Kuzz agent key copies of owner accounts');

-- External agent pairing requests (migration 0090).
CREATE TABLE pairing_requests (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    principal_id      BYTEA NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
    started_device_id UUID REFERENCES devices(id) ON DELETE SET NULL,
    code_hash         BYTEA NOT NULL UNIQUE CHECK (length(code_hash) = 32),
    state             TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'connected')),
    agent_pubkey      BYTEA CHECK (agent_pubkey IS NULL OR length(agent_pubkey) = 32),
    device_pubkey     BYTEA CHECK (device_pubkey IS NULL OR octet_length(device_pubkey) = 33),
    device_name       TEXT CHECK (device_name IS NULL OR char_length(device_name) BETWEEN 1 AND 64),
    device_id         UUID REFERENCES devices(id) ON DELETE SET NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at        TIMESTAMPTZ NOT NULL,
    claimed_at        TIMESTAMPTZ,
    CONSTRAINT pairing_requests_claim_shape CHECK (
        (state = 'open') = (claimed_at IS NULL)
        AND (claimed_at IS NULL) = (agent_pubkey IS NULL)
        AND (claimed_at IS NULL) = (device_pubkey IS NULL)
        AND (state = 'connected' OR (device_name IS NULL AND device_id IS NULL)))
);
CREATE INDEX pairing_requests_principal ON pairing_requests (principal_id, created_at);
CREATE INDEX pairing_requests_expires ON pairing_requests (expires_at);

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('pairing_requests', 'deployment-global external agent pairing requests of owner accounts');

-- ── Relay admin actions (HTTP enforcement state machine) ──────────────────────
-- One row per HTTP report-resolution enforcement action. Tracks the durable
-- state machine from claim → enforcing → succeeded|failed|cancelled.

CREATE TABLE relay_admin_actions (
    id              UUID NOT NULL PRIMARY KEY DEFAULT gen_random_uuid(),
    -- NULL for a report-less direct action (migration 0055).
    report_id       UUID,
    report_community_id UUID NOT NULL,
    -- Client-generated idempotency key (signed in NIP-98 request body).
    request_id      UUID NOT NULL,
    -- Principal who claimed the report.
    actor_pubkey    BYTEA NOT NULL CHECK (length(actor_pubkey) = 32),
    actor_role      TEXT NOT NULL CHECK (actor_role IN ('operator', 'moderator')),
    -- The enforcement action requested.
    action          TEXT NOT NULL,
    reason          TEXT,
    -- Timeout expiration for timeout actions; NULL otherwise.
    timeout_until   TIMESTAMPTZ,
    -- Durable state machine: pending → enforcing → succeeded|failed|cancelled.
    state           TEXT NOT NULL DEFAULT 'pending'
                    CHECK (state IN ('pending', 'enforcing', 'succeeded', 'failed', 'cancelled')),
    -- Step marker: the last durably committed mutation step (NULL = none yet).
    -- Values: 'mutation_committed' (core DB mutation done), 'artifacts_done' (tombstone/notice done).
    step_marker     TEXT CHECK (step_marker IN ('mutation_committed', 'artifacts_done')),
    -- Principal who cancelled a pre-mutation failed action; NULL until cancelled.
    -- Attributes the cancel transition on the action row itself, mirroring
    -- moderation_reports.resolved_by for report resolution.
    cancelled_by    BYTEA CHECK (cancelled_by IS NULL OR length(cancelled_by) = 32),
    -- Error from the last failure, if any.
    error_message   TEXT,
    -- Per-action exclusive lease (migration 0037): fences concurrent same-request
    -- retries and lets the recovery worker claim/re-drive stranded actions.
    action_lease_token      UUID,
    action_lease_expires_at TIMESTAMPTZ,
    -- Authoritative enforcement target (migration 0047): persisted at claim time
    -- so crash-recovery can fire live side effects without re-deriving from mutable
    -- sources. enforcement_target_pubkey is the resolved target pubkey bytes for
    -- kick/ban/timeout actions; NULL for event/blob targets. enforcement_channel_id
    -- is the channel targeted by kick actions; NULL for community-wide actions.
    enforcement_target_pubkey BYTEA
        CHECK (enforcement_target_pubkey IS NULL OR length(enforcement_target_pubkey) = 32),
    enforcement_channel_id  UUID,
    -- Direct actions (migration 0055): deleted event, and requested timeout
    -- duration compared on idempotent retry.
    enforcement_target_event_id BYTEA
        CHECK (enforcement_target_event_id IS NULL OR length(enforcement_target_event_id) = 32),
    timeout_secs    BIGINT CHECK (timeout_secs IS NULL OR timeout_secs > 0),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Report-scoped idempotency: one action per (report, request_id).
    UNIQUE (report_community_id, report_id, request_id),
    CONSTRAINT relay_admin_actions_direct_shape CHECK (
        report_id IS NOT NULL
        OR (action = 'ban' AND enforcement_target_pubkey IS NOT NULL
            AND timeout_secs IS NULL AND timeout_until IS NULL)
        OR (action = 'timeout' AND enforcement_target_pubkey IS NOT NULL
            AND timeout_secs IS NOT NULL AND timeout_until IS NOT NULL)
        OR (action = 'delete' AND enforcement_target_event_id IS NOT NULL
            AND enforcement_target_pubkey IS NOT NULL
            AND timeout_secs IS NULL AND timeout_until IS NULL)
    ),
    FOREIGN KEY (report_community_id, report_id)
        REFERENCES moderation_reports (community_id, id)
);

CREATE INDEX idx_relay_admin_actions_report
    ON relay_admin_actions (report_community_id, report_id);
-- Direct-action idempotency (migration 0055): one per (community, request_id).
CREATE UNIQUE INDEX idx_relay_admin_actions_direct_request
    ON relay_admin_actions (report_community_id, request_id)
    WHERE report_id IS NULL;
CREATE INDEX idx_relay_admin_actions_state
    ON relay_admin_actions (state)
    WHERE state IN ('pending', 'enforcing');
-- Recovery worker (migration 0037): find stranded actions by lease expiry.
CREATE INDEX idx_relay_admin_actions_lease
    ON relay_admin_actions (action_lease_expires_at)
    WHERE state IN ('pending', 'enforcing');

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('relay_admin_actions', 'deployment-global enforcement state machine; community deletion purges rows by report_community_id');

-- ── Relay admin outbox (durable enforcement delivery) ────────────────────────
-- Transactional outbox for durable artifact/notice delivery.

CREATE TABLE relay_admin_outbox (
    id          UUID NOT NULL PRIMARY KEY DEFAULT gen_random_uuid(),
    action_id   UUID NOT NULL REFERENCES relay_admin_actions(id),
    -- Delivery task type: 'tombstone' | 'system_message' | 'reporter_notice'.
    task_type   TEXT NOT NULL,
    -- Task payload (JSON).
    payload     JSONB NOT NULL DEFAULT '{}'::jsonb,
    -- Lease-based delivery: held_by identifies the worker pod.
    held_by     TEXT,
    lease_expires_at TIMESTAMPTZ,
    -- Delivery state: pending → delivered | failed.
    state       TEXT NOT NULL DEFAULT 'pending'
                CHECK (state IN ('pending', 'delivered', 'failed')),
    -- Deduplication key: prevents re-creating an artifact after delivery.
    dedup_key   TEXT UNIQUE,
    error_message TEXT,
    -- Retryable delivery with backoff (migration 0037): failures reschedule via
    -- retry_after rather than terminating immediately.
    attempt_count INT NOT NULL DEFAULT 0,
    retry_after   TIMESTAMPTZ,
    -- Per-claim ownership fence (migration 0038): completion/failure updates
    -- require the token written at claim time, so a stale worker cannot overwrite
    -- a newer worker's terminal update.
    outbox_claim_token UUID,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_relay_admin_outbox_action
    ON relay_admin_outbox (action_id);
CREATE INDEX idx_relay_admin_outbox_pending
    ON relay_admin_outbox (retry_after, created_at)
    WHERE state = 'pending';

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('relay_admin_outbox', 'deployment-global enforcement artifact delivery queue; community deletion purges rows with their action');

-- ── Operator-listener mention delivery ──────────────────────────────────────
-- Listener registrations are deployment-global. The outbox records community
-- provenance for the event, but is intentionally not tenant-owned.

CREATE TABLE operator_listener_pubkeys (
    listener_pubkey BYTEA NOT NULL CHECK (length(listener_pubkey) = 32),
    target_pubkey   BYTEA NOT NULL CHECK (length(target_pubkey) = 32),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (listener_pubkey, target_pubkey)
);
CREATE INDEX operator_listener_pubkeys_target
    ON operator_listener_pubkeys (target_pubkey, listener_pubkey);
CREATE INDEX operator_listener_pubkeys_created_at
    ON operator_listener_pubkeys (created_at);

CREATE TABLE operator_listener_outbox (
    id                UUID NOT NULL PRIMARY KEY DEFAULT gen_random_uuid(),
    listener_pubkey   BYTEA NOT NULL CHECK (length(listener_pubkey) = 32),
    target_pubkey     BYTEA NOT NULL CHECK (length(target_pubkey) = 32),
    community_id      UUID NOT NULL,
    event_id          BYTEA NOT NULL CHECK (length(event_id) = 32),
    event_kind        INTEGER NOT NULL,
    event_created_at  TIMESTAMPTZ NOT NULL,
    state             TEXT NOT NULL DEFAULT 'pending'
                      CHECK (state IN ('pending', 'sending')),
    attempts          INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    lease_until       TIMESTAMPTZ,
    claim_id          UUID,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (listener_pubkey, target_pubkey, community_id, event_id)
);
CREATE INDEX operator_listener_outbox_due
    ON operator_listener_outbox (next_attempt_at, created_at, id)
    WHERE state = 'pending';
CREATE INDEX operator_listener_outbox_recovery
    ON operator_listener_outbox (lease_until, created_at, id)
    WHERE state = 'sending';
CREATE INDEX operator_listener_outbox_created_at
    ON operator_listener_outbox (created_at);

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('operator_listener_pubkeys', 'deployment-global target registrations for operator listeners'),
    ('operator_listener_outbox', 'deployment-global mention delivery queue; community_id is event provenance');

-- ── Relay operator audit (append-only roster mutation trail) ─────────────────
-- One row per PUT/DELETE /operators/{pubkey} mutation. The roster is the
-- deployment-wide root of trust and its mutations overwrite/remove in place;
-- this append-only trail records who granted, elevated, or revoked whom, and
-- when, so privilege changes are as auditable as the enforcement actions those
-- principals perform. Written only inside the upsert/delete transactions; no
-- UPDATE/DELETE path.

CREATE TABLE relay_operator_audit (
    id            UUID NOT NULL PRIMARY KEY DEFAULT gen_random_uuid(),
    actor_pubkey  BYTEA NOT NULL CHECK (length(actor_pubkey) = 32),
    target_pubkey BYTEA NOT NULL CHECK (length(target_pubkey) = 32),
    op            TEXT NOT NULL CHECK (op IN ('grant', 'revoke')),
    prev_role     TEXT CHECK (prev_role IN ('operator', 'moderator')),
    new_role      TEXT CHECK (new_role IN ('operator', 'moderator')),
    -- created_at is wall-clock occurrence time (clock_timestamp()), informational
    -- only — not monotonic, so it never establishes order. `seq` is the sole
    -- chronology key: mutations write their audit row under the serializing lock,
    -- so identity order equals the true privilege chain. Reads use ORDER BY seq.
    created_at    TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    seq           BIGINT GENERATED ALWAYS AS IDENTITY
);

CREATE INDEX idx_relay_operator_audit_target
    ON relay_operator_audit (target_pubkey, seq);

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('relay_operator_audit', 'deployment-global append-only roster mutation audit trail; no community_id intentionally');

-- ── Storage accounting snapshot ─────────────────────────────────────────────
-- Deployment-global singleton produced by the isolated S3 accounting worker.

CREATE TABLE storage_accounting_snapshots (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    snapshot JSONB NOT NULL CHECK (jsonb_typeof(snapshot) = 'object'),
    completed_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    duration_ms BIGINT NOT NULL CHECK (duration_ms >= 0),
    max_objects BIGINT NOT NULL CHECK (max_objects > 0),
    code_sha TEXT NOT NULL CHECK (octet_length(code_sha) BETWEEN 1 AND 128)
);

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('storage_accounting_snapshots', 'deployment-global completed media accounting handoff');

-- NIP-AR current heads and acceptance ledger are independent of event retention.
CREATE TABLE artifact_heads (
    community_id UUID NOT NULL REFERENCES communities(id),
    artifact_id UUID NOT NULL,
    event_id BYTEA NOT NULL CHECK (length(event_id) = 32),
    channel_id UUID NOT NULL,
    artifact_type TEXT NOT NULL,
    root BYTEA,
    deleted BOOLEAN NOT NULL DEFAULT false,
    PRIMARY KEY (community_id, artifact_id)
);
CREATE INDEX artifact_heads_event ON artifact_heads (community_id, event_id);
-- Every accepted revision ID, so replays stay idempotent after redaction or
-- retention.
CREATE TABLE artifact_revisions (
    community_id UUID NOT NULL REFERENCES communities(id),
    event_id BYTEA NOT NULL CHECK (length(event_id) = 32),
    artifact_id UUID NOT NULL,
    PRIMARY KEY (community_id, event_id)
);

SELECT attach_community_write_fence('artifact_heads');
SELECT attach_community_write_fence('artifact_revisions');

-- The relay does not expire events. Any future row retention or partition
-- retirement must skip payloads referenced by `artifact_heads.event_id`
-- (NIP-AR: expiring earlier revisions MUST NOT remove the current revision).

-- Channel history acceptance ordering (migration 0060).
CREATE FUNCTION stamp_channel_event_sequence() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.channel_id IS NULL THEN
        NEW.channel_sequence := 0;
        RETURN NEW;
    END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended(
        'kuzz_channel_membership:' || NEW.community_id::text || ':' || NEW.channel_id::text, 0));
    UPDATE channels SET accepted_sequence = accepted_sequence + 1
    WHERE community_id = NEW.community_id AND id = NEW.channel_id
    RETURNING accepted_sequence INTO NEW.channel_sequence;
    -- Existing event storage permits references to absent channels. They have
    -- no membership and cannot pass a reader's channel authorization.
    IF NOT FOUND THEN NEW.channel_sequence := 0; END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER stamp_channel_event_sequence
BEFORE INSERT ON events
FOR EACH ROW EXECUTE FUNCTION stamp_channel_event_sequence();

CREATE FUNCTION stamp_channel_membership_cutoff() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE' THEN
        IF OLD.community_id IS DISTINCT FROM NEW.community_id
           OR OLD.channel_id IS DISTINCT FROM NEW.channel_id
           OR OLD.pubkey IS DISTINCT FROM NEW.pubkey THEN
            RAISE EXCEPTION 'channel membership identity is immutable'
                USING ERRCODE = 'check_violation';
        END IF;
        -- Active duplicate joins and role changes never advance the boundary.
        IF OLD.removed_at IS NULL OR NEW.removed_at IS NOT NULL THEN
            NEW.join_sequence := OLD.join_sequence;
            NEW.joined_at := OLD.joined_at;
            NEW.join_cutoff_is_legacy := OLD.join_cutoff_is_legacy
                OR (NEW.join_cutoff_is_legacy AND OLD.removed_at IS NULL
                    AND NEW.removed_at IS NULL);
            RETURN NEW;
        END IF;
    END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended(
        'kuzz_channel_membership:' || NEW.community_id::text || ':' || NEW.channel_id::text, 0));
    SELECT accepted_sequence INTO STRICT NEW.join_sequence FROM channels
    WHERE community_id = NEW.community_id AND id = NEW.channel_id;
    NEW.joined_at := clock_timestamp();
    NEW.join_cutoff_is_legacy := false;
    RETURN NEW;
END
$$;

CREATE TRIGGER stamp_channel_membership_cutoff
BEFORE INSERT OR UPDATE ON channel_members
FOR EACH ROW EXECUTE FUNCTION stamp_channel_membership_cutoff();

-- ── Chats: stored latest message, thread name, and unread counts ─────────────
-- Migration 0072 (see the migration for the counting rule and who keeps
-- each value right).

-- A reply whose tags close or reopen its own thread (the desktop's
-- isThreadMarkerReply, matched against the reply's root).
CREATE FUNCTION chat_is_thread_marker(p_tags JSONB, p_root BYTEA)
RETURNS BOOLEAN
LANGUAGE sql IMMUTABLE AS $$
    SELECT p_root IS NOT NULL AND EXISTS (
        SELECT 1 FROM jsonb_array_elements(p_tags) mt
        WHERE mt->>0 IN ('kuzz:thread-closed', 'kuzz:thread-reopened')
          AND lower(mt->>1) = encode(p_root, 'hex'))
$$;

-- since_join history floor: the reader's current join_sequence in a
-- members-only (not `open`) room whose policy is `since_join`, else 0. Rows
-- with channel_sequence <= floor are hidden from that reader. Legacy-stamped
-- memberships keep all history. Channel access itself is checked elsewhere.
-- Room state kinds (channel_kind_is_room_state) are never cut.
CREATE OR REPLACE FUNCTION event_history_sequence(p_seq BIGINT, p_root_seq BIGINT)
RETURNS BIGINT
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE WHEN p_root_seq > 0 AND p_root_seq < p_seq THEN p_root_seq ELSE p_seq END
$$;

CREATE OR REPLACE FUNCTION channel_cuts_history(p_cid UUID, p_chid UUID)
RETURNS BOOLEAN
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT EXISTS (
        SELECT 1 FROM channels hc
         WHERE hc.community_id = p_cid AND hc.id = p_chid
           AND (hc.history_policy = 'since_join' OR hc.group_chat))
$$;

CREATE OR REPLACE FUNCTION channel_history_floor(p_cid UUID, p_chid UUID, p_reader BYTEA)
RETURNS BIGINT
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT CASE WHEN NOT channel_cuts_history(p_cid, p_chid) THEN 0
        ELSE COALESCE((
            SELECT CASE WHEN hm.join_cutoff_is_legacy THEN 0 ELSE hm.join_sequence END
              FROM channel_members hm
             WHERE hm.community_id = p_cid AND hm.channel_id = p_chid
               AND hm.pubkey = p_reader AND hm.removed_at IS NULL),
            (SELECT MIN(CASE WHEN am.join_cutoff_is_legacy THEN 0 ELSE am.join_sequence END)
               FROM channel_members am
               JOIN users au ON au.community_id = am.community_id AND au.pubkey = am.pubkey
               JOIN channels ac ON ac.community_id = am.community_id AND ac.id = am.channel_id
              WHERE am.community_id = p_cid AND am.channel_id = p_chid
                AND am.removed_at IS NULL
                AND au.agent_owner_pubkey = p_reader
                AND (ac.channel_type = 'dm' OR ac.group_chat)),
            9223372036854775807)
    END
$$;

-- True when `reader` must not see the channel event with order number `seq`.
CREATE OR REPLACE FUNCTION channel_event_before_join(p_cid UUID, p_chid UUID, p_seq BIGINT, p_reader BYTEA)
RETURNS BOOLEAN
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT p_chid IS NOT NULL
       AND p_seq <= COALESCE(NULLIF(channel_history_floor(p_cid, p_chid, p_reader), 0), -1)
$$;

-- Current room state (replaceable/addressable kinds, room creation, canvas,
-- goal tree) stays visible to a new member: only the history is cut.
CREATE OR REPLACE FUNCTION channel_kind_is_room_state(p_kind INTEGER)
RETURNS BOOLEAN
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT p_kind IN (9007, 40100, 40110)
        OR p_kind BETWEEN 10000 AND 19999 OR p_kind BETWEEN 30000 AND 39999
$$;

-- True when the reaction, edit, deletion or thread summary (kind) with `tags`
-- names an event (`e` tag) that `reader` must not see. Other kinds: false.
CREATE OR REPLACE FUNCTION channel_event_targets_before_join(
    p_cid UUID, p_chid UUID, p_kind INTEGER, p_tags JSONB, p_reader BYTEA)
RETURNS BOOLEAN
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT CASE
        WHEN p_kind <> ALL (ARRAY[5, 7, 9005, 40003, 39005]) THEN false
        WHEN p_chid IS NOT NULL AND channel_history_floor(p_cid, p_chid, p_reader) = 0 THEN false
        -- A deletion is hidden only when everything it deletes is: it also
        -- removes visible messages from the reader's view.
        WHEN p_kind IN (5, 9005) THEN COALESCE((
            SELECT bool_and(channel_event_before_join(te.community_id, te.channel_id,
                    event_history_sequence(te.channel_sequence, te.thread_root_sequence),
                    p_reader))
              FROM jsonb_array_elements(p_tags) t
              JOIN events te ON te.community_id = p_cid
               AND te.id = CASE WHEN t->>1 ~ '^[0-9a-fA-F]{64}$' THEN decode(t->>1, 'hex') END
             WHERE t->>0 = 'e'), false)
        ELSE EXISTS (
            SELECT 1 FROM jsonb_array_elements(p_tags) t
            JOIN events te ON te.community_id = p_cid
             AND te.id = CASE WHEN t->>1 ~ '^[0-9a-fA-F]{64}$' THEN decode(t->>1, 'hex') END
            WHERE t->>0 = 'e'
              AND channel_event_before_join(te.community_id, te.channel_id,
                    event_history_sequence(te.channel_sequence, te.thread_root_sequence),
                    p_reader))
    END
$$;

-- A DM between exactly two people (migration 0073). DM participant sets never
-- change, so the member row count is the participant count.
CREATE FUNCTION dm_is_one_to_one(p_cid UUID, p_chid UUID)
RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT EXISTS (SELECT 1 FROM channels c
                    WHERE c.community_id = p_cid AND c.id = p_chid AND c.channel_type = 'dm')
       AND (SELECT count(*) FROM channel_members m
             WHERE m.community_id = p_cid AND m.channel_id = p_chid) = 2
$$;

-- Unread and unread-mention counts of a room's main timeline for one reader.
-- In a 1:1 DM, messages received during one of the reader's block periods of
-- their author do not count (migration 0073).
CREATE FUNCTION chat_room_unread(
    p_cid UUID, p_chid UUID, p_reader BYTEA, p_after BIGINT,
    OUT unread INTEGER, OUT mentions INTEGER)
LANGUAGE plpgsql STABLE AS $$
DECLARE
    v_dm BOOLEAN;
    v_reader_hex TEXT := encode(p_reader, 'hex');
    v_after BIGINT := COALESCE(p_after, 0);
    v_floor BIGINT;
BEGIN
    v_dm := dm_is_one_to_one(p_cid, p_chid);
    -- (The fail-closed floor of a missing membership is ignored: the tally
    -- trigger counts a row before it is in the table and passes its own cut.)
    v_floor := COALESCE(NULLIF(channel_history_floor(p_cid, p_chid, p_reader),
                               9223372036854775807), 0);
    v_after := GREATEST(v_after, v_floor);
    SELECT count(*) INTO unread FROM (
        SELECT 1 FROM events e
         WHERE e.community_id = p_cid AND e.channel_id = p_chid
           AND e.channel_sequence > v_after
           AND (v_floor = 0 OR e.thread_root_sequence = 0 OR e.thread_root_sequence > v_floor)
           AND e.kind IN (9, 40002, 45001, 45003) AND e.deleted_at IS NULL
           AND e.pubkey <> p_reader
           AND NOT EXISTS (
               SELECT 1 FROM thread_metadata r
                WHERE r.community_id = p_cid AND r.event_id = e.id AND r.depth > 0
                  AND (NOT r.broadcast OR chat_is_thread_marker(e.tags, r.root_event_id)))
           AND NOT (v_dm AND EXISTS (
               SELECT 1 FROM user_block_periods bp
                WHERE bp.community_id = p_cid AND bp.blocker = p_reader
                  AND bp.blocked = e.pubkey AND bp.started_at <= e.received_at
                  AND (bp.ended_at IS NULL OR e.received_at < bp.ended_at)))
         LIMIT 100) n;
    SELECT count(*) INTO mentions FROM (
        SELECT 1 FROM events e
         WHERE e.community_id = p_cid AND e.channel_id = p_chid
           AND e.channel_sequence > v_after
           AND (v_floor = 0 OR e.thread_root_sequence = 0 OR e.thread_root_sequence > v_floor)
           AND e.kind IN (9, 40002, 45001, 45003) AND e.deleted_at IS NULL
           AND e.pubkey <> p_reader
           AND NOT EXISTS (
               SELECT 1 FROM thread_metadata r
                WHERE r.community_id = p_cid AND r.event_id = e.id AND r.depth > 0
                  AND (NOT r.broadcast OR chat_is_thread_marker(e.tags, r.root_event_id)))
           AND NOT (v_dm AND EXISTS (
               SELECT 1 FROM user_block_periods bp
                WHERE bp.community_id = p_cid AND bp.blocker = p_reader
                  AND bp.blocked = e.pubkey AND bp.started_at <= e.received_at
                  AND (bp.ended_at IS NULL OR e.received_at < bp.ended_at)))
           AND EXISTS (
               SELECT 1 FROM event_mentions em
                WHERE em.community_id = p_cid AND em.pubkey_hex = v_reader_hex
                  AND em.event_id = e.id)
         LIMIT 100) n;
END
$$;

-- Unread and unread-mention counts of a thread for one reader. `p_chid` is
-- the thread's room (it decides whether the 1:1 DM block rule applies).
CREATE FUNCTION chat_thread_unread(
    p_cid UUID, p_chid UUID, p_root BYTEA, p_reader BYTEA, p_after BIGINT,
    OUT unread INTEGER, OUT mentions INTEGER)
LANGUAGE plpgsql STABLE AS $$
DECLARE
    v_dm BOOLEAN;
    v_reader_hex TEXT := encode(p_reader, 'hex');
BEGIN
    v_dm := dm_is_one_to_one(p_cid, p_chid);
    SELECT count(*) INTO unread FROM (
        SELECT 1 FROM thread_metadata r
          JOIN events e ON e.community_id = r.community_id
           AND e.created_at = r.event_created_at AND e.id = r.event_id
         WHERE r.community_id = p_cid AND r.root_event_id = p_root AND r.depth > 0
           AND e.channel_sequence > COALESCE(p_after, 0)
           AND e.kind IN (9, 40002, 45001, 45003) AND e.deleted_at IS NULL
           AND e.pubkey <> p_reader
           AND NOT chat_is_thread_marker(e.tags, p_root)
           AND NOT (v_dm AND EXISTS (
               SELECT 1 FROM user_block_periods bp
                WHERE bp.community_id = p_cid AND bp.blocker = p_reader
                  AND bp.blocked = e.pubkey AND bp.started_at <= e.received_at
                  AND (bp.ended_at IS NULL OR e.received_at < bp.ended_at)))
         LIMIT 100) n;
    SELECT count(*) INTO mentions FROM (
        SELECT 1 FROM thread_metadata r
          JOIN events e ON e.community_id = r.community_id
           AND e.created_at = r.event_created_at AND e.id = r.event_id
         WHERE r.community_id = p_cid AND r.root_event_id = p_root AND r.depth > 0
           AND e.channel_sequence > COALESCE(p_after, 0)
           AND e.kind IN (9, 40002, 45001, 45003) AND e.deleted_at IS NULL
           AND e.pubkey <> p_reader
           AND NOT chat_is_thread_marker(e.tags, p_root)
           AND NOT (v_dm AND EXISTS (
               SELECT 1 FROM user_block_periods bp
                WHERE bp.community_id = p_cid AND bp.blocker = p_reader
                  AND bp.blocked = e.pubkey AND bp.started_at <= e.received_at
                  AND (bp.ended_at IS NULL OR e.received_at < bp.ended_at)))
           AND EXISTS (
               SELECT 1 FROM event_mentions em
                WHERE em.community_id = p_cid AND em.pubkey_hex = v_reader_hex
                  AND em.event_id = e.id)
         LIMIT 100) n;
END
$$;

-- Latest live main-timeline message of a room (not a marker).
CREATE FUNCTION chat_room_latest_id(p_cid UUID, p_chid UUID)
RETURNS BYTEA
LANGUAGE sql STABLE AS $$
    SELECT e.id FROM events e
     WHERE e.community_id = p_cid AND e.channel_id = p_chid
       AND e.kind IN (9, 40002, 45001, 45003) AND e.deleted_at IS NULL
       AND NOT EXISTS (
           SELECT 1 FROM thread_metadata r
            WHERE r.community_id = p_cid AND r.event_id = e.id AND r.depth > 0
              AND (NOT r.broadcast OR chat_is_thread_marker(e.tags, r.root_event_id)))
     ORDER BY e.channel_sequence DESC LIMIT 1
$$;

-- Latest live reply of a thread (not a marker).
CREATE FUNCTION chat_thread_latest_id(p_cid UUID, p_root BYTEA)
RETURNS BYTEA
LANGUAGE sql STABLE AS $$
    SELECT e.id FROM thread_metadata r
      JOIN events e ON e.community_id = r.community_id
       AND e.created_at = r.event_created_at AND e.id = r.event_id
     WHERE r.community_id = p_cid AND r.root_event_id = p_root AND r.depth > 0
       AND e.kind IN (9, 40002, 45001, 45003) AND e.deleted_at IS NULL
       AND NOT chat_is_thread_marker(e.tags, p_root)
     ORDER BY e.channel_sequence DESC LIMIT 1
$$;

-- Newest thread name (kind 40009) in the thread's room; an empty name clears.
CREATE FUNCTION chat_thread_name(p_cid UUID, p_chid UUID, p_root BYTEA)
RETURNS TEXT
LANGUAGE sql STABLE AS $$
    SELECT NULLIF(e.content, '') FROM events e
     WHERE e.community_id = p_cid AND e.channel_id = p_chid AND e.kind = 40009
       AND e.deleted_at IS NULL
       AND e.tags @> jsonb_build_array(jsonb_build_array('e', encode(p_root, 'hex')))
     ORDER BY e.created_at DESC, e.id DESC LIMIT 1
$$;

-- Point a room's stored latest id at its latest live main message. With
-- `p_only_if`, only when it currently points at that event. The sort time
-- and sequence (main_last_seq / main_last_at) never move back.
CREATE FUNCTION chat_set_room_latest(p_cid UUID, p_chid UUID, p_only_if BYTEA)
RETURNS VOID
LANGUAGE sql AS $$
    UPDATE channels SET main_last_event_id = chat_room_latest_id(p_cid, p_chid)
     WHERE community_id = p_cid AND id = p_chid
       AND (p_only_if IS NULL OR main_last_event_id = p_only_if)
$$;

-- Recount current members of a room, optionally only those whose read
-- position is below `p_below` and who are not `p_skip`.
CREATE FUNCTION chat_recount_room_members(
    p_cid UUID, p_chid UUID, p_below BIGINT, p_skip BYTEA)
RETURNS VOID
LANGUAGE sql AS $$
    UPDATE channel_members cm
       SET (unread_count, unread_mentions) = (
           SELECT u.unread, u.mentions
             FROM chat_room_unread(cm.community_id, cm.channel_id, cm.pubkey, cm.read_seq) u)
     WHERE cm.community_id = p_cid AND cm.channel_id = p_chid AND cm.removed_at IS NULL
       AND (p_below IS NULL OR COALESCE(cm.read_seq, 0) < p_below)
       AND (p_skip IS NULL OR cm.pubkey <> p_skip)
$$;

-- Recount follow rows: of one thread (`p_root`), or of one room (`p_chid`),
-- optionally one reader's, below a read position, and not `p_skip`.
CREATE FUNCTION chat_recount_thread_follows(
    p_cid UUID, p_root BYTEA, p_chid UUID, p_reader BYTEA, p_below BIGINT, p_skip BYTEA)
RETURNS VOID
LANGUAGE sql AS $$
    UPDATE thread_follows tf
       SET (unread_count, unread_mentions) = (
           SELECT u.unread, u.mentions
             FROM chat_thread_unread(tf.community_id, tf.channel_id, tf.root_event_id,
                                     tf.pubkey, tf.read_seq) u)
     WHERE tf.community_id = p_cid
       AND (p_root IS NULL OR tf.root_event_id = p_root)
       AND (p_chid IS NULL OR tf.channel_id = p_chid)
       AND (p_reader IS NULL OR tf.pubkey = p_reader)
       AND (p_below IS NULL OR COALESCE(tf.read_seq, 0) < p_below)
       AND (p_skip IS NULL OR tf.pubkey <> p_skip)
$$;

-- A read position at or past the room's last accepted sequence has nothing
-- unread: no committed message has a higher sequence (a message still being
-- written gets its +1 when it commits). The common case (read to the end,
-- or the author's own message) skips counting.
CREATE FUNCTION chat_read_to_end(p_cid UUID, p_chid UUID, p_read_seq BIGINT)
RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT p_read_seq IS NOT NULL AND EXISTS (
        SELECT 1 FROM channels c
         WHERE c.community_id = p_cid AND c.id = p_chid
           AND c.accepted_sequence <= p_read_seq)
$$;

-- channel_members: recount on insert, when the read position moves, and on
-- rejoin. Runs after stamp_channel_membership_cutoff (BEFORE triggers fire in
-- name order), which holds the room lock for joins, so the count sees every
-- message committed before the join.
CREATE FUNCTION tally_channel_member_unread() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
DECLARE
    v_after BIGINT := NEW.read_seq;
BEGIN
    IF NEW.removed_at IS NOT NULL THEN
        RETURN NEW;
    END IF;
    IF TG_OP = 'UPDATE' AND NEW.read_seq IS NOT DISTINCT FROM OLD.read_seq
       AND OLD.removed_at IS NULL
       AND NEW.join_cutoff_is_legacy IS NOT DISTINCT FROM OLD.join_cutoff_is_legacy THEN
        RETURN NEW;
    END IF;
    IF chat_read_to_end(NEW.community_id, NEW.channel_id, NEW.read_seq) THEN
        NEW.unread_count := 0;
        NEW.unread_mentions := 0;
    ELSE
        -- Count only after this (re)join. The table row is not this
        -- membership yet, so channel_history_floor cannot see it.
        IF NOT NEW.join_cutoff_is_legacy
           AND channel_cuts_history(NEW.community_id, NEW.channel_id) THEN
            v_after := GREATEST(COALESCE(v_after, 0), NEW.join_sequence);
        END IF;
        SELECT u.unread, u.mentions INTO NEW.unread_count, NEW.unread_mentions
          FROM chat_room_unread(NEW.community_id, NEW.channel_id, NEW.pubkey, v_after) u;
    END IF;
    IF TG_OP = 'UPDATE' AND OLD.removed_at IS NOT NULL THEN
        PERFORM chat_recount_thread_follows(
            NEW.community_id, NULL, NEW.channel_id, NEW.pubkey, NULL, NULL);
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER tally_channel_member_unread_insert
BEFORE INSERT ON channel_members
FOR EACH ROW EXECUTE FUNCTION tally_channel_member_unread();

CREATE TRIGGER tally_channel_member_unread_update
BEFORE UPDATE ON channel_members
FOR EACH ROW
WHEN (OLD.read_seq IS DISTINCT FROM NEW.read_seq
      OR (OLD.removed_at IS NOT NULL AND NEW.removed_at IS NULL))
EXECUTE FUNCTION tally_channel_member_unread();

-- thread_follows: recount on insert and when the read position moves.
CREATE FUNCTION tally_thread_follow_unread() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND NEW.read_seq IS NOT DISTINCT FROM OLD.read_seq THEN
        RETURN NEW;
    END IF;
    IF chat_read_to_end(NEW.community_id, NEW.channel_id, NEW.read_seq) THEN
        NEW.unread_count := 0;
        NEW.unread_mentions := 0;
    ELSE
        SELECT u.unread, u.mentions INTO NEW.unread_count, NEW.unread_mentions
          FROM chat_thread_unread(NEW.community_id, NEW.channel_id, NEW.root_event_id,
                                  NEW.pubkey, NEW.read_seq) u;
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER tally_thread_follow_unread_insert
BEFORE INSERT ON thread_follows
FOR EACH ROW EXECUTE FUNCTION tally_thread_follow_unread();

CREATE TRIGGER tally_thread_follow_unread_update
BEFORE UPDATE ON thread_follows
FOR EACH ROW
WHEN (OLD.read_seq IS DISTINCT FROM NEW.read_seq)
EXECUTE FUNCTION tally_thread_follow_unread();

-- A message (or thread name) deleted or restored: recount the rows that had
-- it unread and move a stored latest id or name that pointed at it.
CREATE FUNCTION chat_state_after_message_delete() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
DECLARE
    v_root BYTEA;
    v_broadcast BOOLEAN;
    v_marker BOOLEAN := FALSE;
BEGIN
    -- Same filter as the trigger's WHEN clause, kept here as well so the
    -- function stays correct if a schema tool drops the clause.
    IF OLD.deleted_at IS NOT DISTINCT FROM NEW.deleted_at OR NEW.channel_id IS NULL
       OR NEW.kind NOT IN (9, 40002, 45001, 45003, 40009) THEN
        RETURN NULL;
    END IF;
    -- Communities being deleted are write-fenced; their rows are going away.
    IF NOT EXISTS (SELECT 1 FROM communities
                    WHERE id = NEW.community_id AND deletion_state = 'active') THEN
        RETURN NULL;
    END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended(
        'kuzz_channel_membership:' || NEW.community_id::text || ':' || NEW.channel_id::text, 0));

    IF NEW.kind = 40009 THEN
        UPDATE thread_state ts
           SET name = chat_thread_name(ts.community_id, ts.channel_id, ts.root_event_id)
         WHERE ts.community_id = NEW.community_id AND ts.channel_id = NEW.channel_id
           AND ts.root_event_id IN (
               SELECT decode(t->>1, 'hex') FROM jsonb_array_elements(NEW.tags) t
                WHERE t->>0 = 'e' AND t->>1 ~ '^[0-9a-f]{64}$');
        RETURN NULL;
    END IF;

    SELECT r.root_event_id, r.broadcast INTO v_root, v_broadcast
      FROM thread_metadata r
     WHERE r.community_id = NEW.community_id AND r.event_id = NEW.id AND r.depth > 0
     LIMIT 1;
    IF v_root IS NOT NULL THEN
        v_marker := chat_is_thread_marker(NEW.tags, v_root);
    END IF;

    IF v_root IS NULL OR (v_broadcast AND NOT v_marker) THEN
        PERFORM chat_recount_room_members(
            NEW.community_id, NEW.channel_id, NEW.channel_sequence, NEW.pubkey);
        -- Deleted: only when it was the latest. Restored: it may be newer.
        PERFORM chat_set_room_latest(NEW.community_id, NEW.channel_id,
            CASE WHEN NEW.deleted_at IS NOT NULL THEN NEW.id END);
    END IF;
    IF v_root IS NOT NULL AND NOT v_marker THEN
        PERFORM chat_recount_thread_follows(
            NEW.community_id, v_root, NULL, NULL, NEW.channel_sequence, NEW.pubkey);
        UPDATE thread_state
           SET last_event_id = chat_thread_latest_id(NEW.community_id, v_root)
         WHERE community_id = NEW.community_id AND root_event_id = v_root
           AND (NEW.deleted_at IS NULL OR last_event_id = NEW.id);
    END IF;
    RETURN NULL;
END
$$;

CREATE TRIGGER chat_state_after_message_delete
AFTER UPDATE OF deleted_at ON events
FOR EACH ROW
WHEN (OLD.deleted_at IS DISTINCT FROM NEW.deleted_at
      AND NEW.channel_id IS NOT NULL
      AND NEW.kind IN (9, 40002, 45001, 45003, 40009))
EXECUTE FUNCTION chat_state_after_message_delete();

-- A thread name (kind 40009) written by any path: store the thread's newest
-- name. The insert already holds the room lock (sequence trigger).
CREATE FUNCTION chat_state_after_thread_name() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    -- Same filter as the trigger's WHEN clause (see the delete trigger).
    IF NEW.kind <> 40009 OR NEW.channel_id IS NULL OR NEW.deleted_at IS NOT NULL THEN
        RETURN NULL;
    END IF;
    INSERT INTO thread_state (community_id, root_event_id, channel_id, name)
    SELECT NEW.community_id, root.id, NEW.channel_id,
           chat_thread_name(NEW.community_id, NEW.channel_id, root.id)
      FROM (SELECT DISTINCT decode(t->>1, 'hex') AS id
              FROM jsonb_array_elements(NEW.tags) t
             WHERE t->>0 = 'e' AND t->>1 ~ '^[0-9a-f]{64}$') root
     WHERE EXISTS (SELECT 1 FROM events r
                    WHERE r.community_id = NEW.community_id AND r.id = root.id
                      AND r.channel_id = NEW.channel_id)
    ON CONFLICT (community_id, root_event_id) DO UPDATE
    SET name = chat_thread_name(thread_state.community_id, thread_state.channel_id,
                                thread_state.root_event_id);
    RETURN NULL;
END
$$;

CREATE TRIGGER chat_state_after_thread_name
AFTER INSERT ON events
FOR EACH ROW
WHEN (NEW.kind = 40009 AND NEW.channel_id IS NOT NULL AND NEW.deleted_at IS NULL)
EXECUTE FUNCTION chat_state_after_thread_name();

-- A block added or removed: recount the blocker's rows in DMs the other
-- person is (or was) in. Each DM's room lock is taken first (in channel id
-- order), so a message from the other person commits entirely before the
-- recount or is written after the block change is visible.
CREATE FUNCTION chat_state_after_block_change() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
DECLARE
    v_cid UUID;
    v_blocker BYTEA;
    v_blocked BYTEA;
    v_chid UUID;
BEGIN
    IF TG_OP = 'DELETE' THEN
        v_cid := OLD.community_id; v_blocker := OLD.blocker; v_blocked := OLD.blocked;
    ELSE
        v_cid := NEW.community_id; v_blocker := NEW.blocker; v_blocked := NEW.blocked;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM communities
                    WHERE id = v_cid AND deletion_state = 'active') THEN
        RETURN NULL;
    END IF;
    FOR v_chid IN
        SELECT c.id FROM channels c
         WHERE c.community_id = v_cid AND c.channel_type = 'dm'
           AND EXISTS (SELECT 1 FROM channel_members a
                        WHERE a.community_id = v_cid AND a.channel_id = c.id
                          AND a.pubkey = v_blocker)
           AND EXISTS (SELECT 1 FROM channel_members o
                        WHERE o.community_id = v_cid AND o.channel_id = c.id
                          AND o.pubkey = v_blocked)
         ORDER BY c.id
    LOOP
        PERFORM pg_advisory_xact_lock(hashtextextended(
            'kuzz_channel_membership:' || v_cid::text || ':' || v_chid::text, 0));
    END LOOP;
    UPDATE channel_members cm
       SET (unread_count, unread_mentions) = (
           SELECT u.unread, u.mentions
             FROM chat_room_unread(cm.community_id, cm.channel_id, cm.pubkey, cm.read_seq) u)
      FROM channels c
     WHERE c.community_id = cm.community_id AND c.id = cm.channel_id AND c.channel_type = 'dm'
       AND cm.community_id = v_cid AND cm.pubkey = v_blocker AND cm.removed_at IS NULL
       AND EXISTS (SELECT 1 FROM channel_members o
                    WHERE o.community_id = cm.community_id AND o.channel_id = cm.channel_id
                      AND o.pubkey = v_blocked);
    UPDATE thread_follows tf
       SET (unread_count, unread_mentions) = (
           SELECT u.unread, u.mentions
             FROM chat_thread_unread(tf.community_id, tf.channel_id, tf.root_event_id,
                                     tf.pubkey, tf.read_seq) u)
      FROM channels c
     WHERE c.community_id = tf.community_id AND c.id = tf.channel_id AND c.channel_type = 'dm'
       AND tf.community_id = v_cid AND tf.pubkey = v_blocker
       AND EXISTS (SELECT 1 FROM channel_members o
                    WHERE o.community_id = tf.community_id AND o.channel_id = tf.channel_id
                      AND o.pubkey = v_blocked);
    RETURN NULL;
END
$$;

CREATE TRIGGER chat_state_after_block_change
AFTER INSERT OR DELETE ON user_blocks
FOR EACH ROW EXECUTE FUNCTION chat_state_after_block_change();

-- Fill every stored value of one room from its messages, under the room lock.
-- Also run by the chat-state backfill command for each room it visits.
CREATE FUNCTION refill_chat_stored_state_channel(p_cid UUID, p_chid UUID)
RETURNS VOID
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(hashtextextended(
        'kuzz_channel_membership:' || p_cid::text || ':' || p_chid::text, 0));
    PERFORM chat_set_room_latest(p_cid, p_chid, NULL);
    -- Named threads with no state row yet (no reply so far).
    INSERT INTO thread_state (community_id, root_event_id, channel_id)
    SELECT DISTINCT p_cid, decode(t->>1, 'hex'), p_chid
      FROM events e
      CROSS JOIN LATERAL jsonb_array_elements(e.tags) t
     WHERE e.community_id = p_cid AND e.channel_id = p_chid AND e.kind = 40009
       AND e.deleted_at IS NULL AND t->>0 = 'e' AND t->>1 ~ '^[0-9a-f]{64}$'
       AND EXISTS (SELECT 1 FROM events root
                    WHERE root.community_id = p_cid AND root.id = decode(t->>1, 'hex')
                      AND root.channel_id = p_chid)
    ON CONFLICT (community_id, root_event_id) DO NOTHING;
    UPDATE thread_state ts
       SET last_event_id = chat_thread_latest_id(ts.community_id, ts.root_event_id),
           name = chat_thread_name(ts.community_id, ts.channel_id, ts.root_event_id)
     WHERE ts.community_id = p_cid AND ts.channel_id = p_chid;
    PERFORM chat_recount_room_members(p_cid, p_chid, NULL, NULL);
    PERFORM chat_recount_thread_follows(p_cid, NULL, p_chid, NULL, NULL, NULL);
END
$$;

-- Every live room of every community not being deleted. Returns the number
-- of rooms filled.
CREATE FUNCTION refill_chat_stored_state() RETURNS BIGINT
LANGUAGE plpgsql AS $$
DECLARE
    room RECORD;
    filled BIGINT := 0;
BEGIN
    FOR room IN
        SELECT c.community_id AS cid, c.id AS chid
          FROM channels c
          JOIN communities co ON co.id = c.community_id AND co.deletion_state = 'active'
         WHERE c.deleted_at IS NULL
         ORDER BY c.community_id, c.id
    LOOP
        PERFORM refill_chat_stored_state_channel(room.cid, room.chid);
        filled := filled + 1;
    END LOOP;
    RETURN filled;
END
$$;

CREATE OR REPLACE FUNCTION stamp_thread_root_sequence() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
DECLARE
    v_root BIGINT;
BEGIN
    IF NEW.depth <= 0 OR NEW.root_event_id IS NULL THEN
        RETURN NULL;
    END IF;
    SELECT r.channel_sequence INTO v_root FROM events r
     WHERE r.community_id = NEW.community_id AND r.id = NEW.root_event_id
       AND r.channel_id = NEW.channel_id
       AND (NEW.root_event_created_at IS NULL OR r.created_at = NEW.root_event_created_at)
     LIMIT 1;
    IF COALESCE(v_root, 0) > 0 THEN
        UPDATE events SET thread_root_sequence = v_root
         WHERE community_id = NEW.community_id AND id = NEW.event_id
           AND created_at = NEW.event_created_at AND thread_root_sequence <> v_root;
    END IF;
    RETURN NULL;
END
$$;

CREATE TRIGGER stamp_thread_root_sequence
AFTER INSERT ON thread_metadata
FOR EACH ROW EXECUTE FUNCTION stamp_thread_root_sequence();
