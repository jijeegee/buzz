-- Region propagation outbox (global regions stage S4-5).
--
-- Logouts, token revocations and disconnects close connections only in the
-- region where they happen (its Redis). Each one also writes a row here per
-- other region; a worker delivers it over the region link, and the target
-- region applies it to its own connections. Applying is idempotent (close
-- what matches), so retries and duplicates are safe. A delivered row is
-- deleted. Rows of this database's region only: never copied between regions.
CREATE TABLE region_outbox (
    id BIGSERIAL PRIMARY KEY,
    target_region TEXT NOT NULL,
    -- The community named in the link call (every region serves it).
    community_id UUID NOT NULL,
    topic TEXT NOT NULL,
    payload JSONB NOT NULL,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    retry_after TIMESTAMPTZ,
    held_by TEXT,
    lease_expires_at TIMESTAMPTZ,
    claim_token UUID,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX region_outbox_due ON region_outbox (retry_after NULLS FIRST, id);

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('region_outbox', 'region-local delivery queue to other regions');
