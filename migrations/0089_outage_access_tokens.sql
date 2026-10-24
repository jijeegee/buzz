-- Access tokens a region gives out while the primary is down (global regions
-- stage S5-6).
--
-- Refreshing a session writes the global tables, so it runs in the primary.
-- When the primary cannot be reached and this region's copy has fallen
-- behind, a region answers a refresh itself: it checks the refresh token
-- against its copy and gives out a short access token recorded here. Nothing
-- global is written; the refresh token is not rotated.
--
-- The table is this region's own and never copied. The token check reads it
-- after `access_tokens`, joined to the copied session and account, so a
-- revoked session or a disabled account stops these tokens like any other.
-- No foreign keys: a copy's replication would not cascade a deletion into a
-- local table, and the join already ignores a row whose session is gone.
CREATE TABLE outage_access_tokens (
    token_hash   BYTEA PRIMARY KEY CHECK (length(token_hash) = 32),
    principal_id BYTEA NOT NULL CHECK (length(principal_id) = 32),
    session_id   UUID NOT NULL,
    -- The refresh token it was given for, to bound how many are given out.
    refresh_hash BYTEA NOT NULL CHECK (length(refresh_hash) = 32),
    expires_at   TIMESTAMPTZ NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX outage_access_tokens_expiry ON outage_access_tokens (expires_at);
CREATE INDEX outage_access_tokens_refresh ON outage_access_tokens (refresh_hash, created_at);

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('outage_access_tokens', 'region-local access tokens given out while the primary is down');
