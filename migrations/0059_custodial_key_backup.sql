-- Google sessions authorize backup recovery, not a replacement messaging identity.
ALTER TABLE principals ADD COLUMN identity_mode TEXT NOT NULL DEFAULT 'token'
    CONSTRAINT principals_identity_mode CHECK (identity_mode IN ('token', 'key_backup'));

CREATE TABLE account_key_backups (
    account_id BYTEA PRIMARY KEY REFERENCES principals(id),
    pubkey BYTEA NOT NULL UNIQUE CHECK (length(pubkey) = 32),
    version SMALLINT NOT NULL CHECK (version = 1),
    key_id TEXT NOT NULL CHECK (key_id ~ '^[A-Za-z0-9_-]{1,64}$'),
    nonce BYTEA NOT NULL CHECK (length(nonce) = 12),
    ciphertext BYTEA NOT NULL CHECK (length(ciphertext) = 48),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('account_key_backups', 'deployment-global immutable Google account to Nostr key backup binding')
ON CONFLICT (table_name) DO NOTHING;
