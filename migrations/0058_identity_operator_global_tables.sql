-- Register the centralized-identity tables from migration 0056 as
-- deployment-global: they intentionally carry no community_id. The hardcoded
-- parser list in crates/buzz-db/src/runtime/migration.rs mirrors this set.
INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('principals', 'deployment-global server accounts (users, bots, relay); no community_id intentionally'),
    ('identities', 'deployment-global external OIDC identities linked to principals'),
    ('devices', 'deployment-global login devices of a principal'),
    ('sessions', 'deployment-global login sessions per device'),
    ('refresh_tokens', 'deployment-global refresh-token rotation history'),
    ('bots', 'deployment-global bot registrations owned by a user principal'),
    ('access_tokens', 'deployment-global opaque access-token hashes');
