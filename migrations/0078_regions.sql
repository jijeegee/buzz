-- Regions of this deployment (global regions stage S2).
--
-- One row per region (`kr`, `jp`, ...). Each relay records its own region at
-- startup from `KUZZ_REGION_ID`. Room and user home markers refer to these
-- ids. At most one region is primary: it runs the deployment-wide jobs and
-- holds the global lists.
--
-- Deployment-global, not tenant data: a region serves every community.
CREATE TABLE regions (
    id TEXT PRIMARY KEY CHECK (id ~ '^[a-z][a-z0-9-]{0,15}$'),
    connect_url TEXT NOT NULL,
    is_primary BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX regions_one_primary ON regions (is_primary) WHERE is_primary;

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('regions', 'deployment regions; a region serves every community');
