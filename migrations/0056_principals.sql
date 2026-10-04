-- Centralized identity (Phase 0): server accounts, devices, sessions, bots and
-- opaque access tokens. Additive only — key-based auth keeps working and every
-- surface that reads these tables is gated by AUTH_TOKEN_ENABLED (default off).
--
-- All tables are deployment-global (no community_id): a principal spans every
-- community exactly like today's Nostr pubkey does. They are registered in
-- _operator_global_tables by migration 0058.
--
-- Principal ids are 32-byte values that are also valid secp256k1 x-only public
-- keys (see buzz_core::principal::PrincipalId). SQL cannot check curve
-- membership, so the store layer is the only writer and only accepts a
-- validated PrincipalId.

CREATE TABLE principals (
    id            BYTEA PRIMARY KEY CHECK (length(id) = 32),
    kind          TEXT NOT NULL CHECK (kind IN ('user', 'bot', 'relay')),
    display_name  TEXT NOT NULL DEFAULT '',
    avatar_url    TEXT,
    username      TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    disabled_at   TIMESTAMPTZ,
    purge_after   TIMESTAMPTZ
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

CREATE TABLE devices (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    principal_id  BYTEA NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    platform      TEXT NOT NULL CHECK (platform IN ('desktop', 'mobile', 'web', 'cli')),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at    TIMESTAMPTZ
);
CREATE INDEX devices_principal ON devices (principal_id);

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
