-- Account purge: 30 days after a user deletes their account, the relay
-- removes what identifies them (sign-ins, devices, key backups, profile,
-- friend and block lists) and keeps the messages others saw. The principal
-- row stays as a disabled tombstone (`purged_at` set) so its id stays
-- refused; `purged_account_keys` keeps a purged account's messaging key
-- refused once its key backup row is gone.
--
-- Additive only.
ALTER TABLE principals ADD COLUMN purged_at TIMESTAMPTZ;

CREATE INDEX principals_purge_due
    ON principals (purge_after) WHERE purge_after IS NOT NULL AND purged_at IS NULL;

CREATE TABLE purged_account_keys (
    pubkey     BYTEA PRIMARY KEY CHECK (length(pubkey) = 32),
    purged_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('purged_account_keys', 'deployment-global messaging keys of purged accounts, refused forever')
ON CONFLICT (table_name) DO NOTHING;
