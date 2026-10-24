-- Sign in with Apple: the refresh token Apple returns at sign-in, kept only to
-- revoke the app's access to the Apple ID when the account is deleted (App
-- Store review requirement). Encrypted under the key-backup master key with
-- its own AAD domain (kuzz/provider-revoke-token/v1, binding provider,
-- subject and master-key label). Keyed by the external sign-in, not the
-- account, so a token can be stored before the account exists (signup
-- confirmation); account deletion revokes it at Apple and deletes the row.
--
-- Additive only.
CREATE TABLE provider_revoke_tokens (
    provider    TEXT NOT NULL,
    subject     TEXT NOT NULL,
    key_id      TEXT NOT NULL CHECK (key_id ~ '^[A-Za-z0-9_-]{1,64}$'),
    nonce       BYTEA NOT NULL CHECK (length(nonce) = 12),
    ciphertext  BYTEA NOT NULL CHECK (length(ciphertext) BETWEEN 17 AND 4112),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (provider, subject)
);

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('provider_revoke_tokens', 'deployment-global encrypted provider revoke tokens of external sign-ins')
ON CONFLICT (table_name) DO NOTHING;
