-- Device keys (device-bound keys plan §2, PR A1a). Every install keeps one
-- P-256 key pair (TPM, Secure Enclave or Android Keystore where available)
-- and signs each use of its message keys and tokens with it, so a key or
-- token copied to another machine is useless. One device key may sign in to
-- several accounts (one row per account).
--
-- Additive only: the previous relay image ignores the new columns and table,
-- and the install-id index is unchanged (rolling back the image needs
-- KUZZ_AUTO_MIGRATE off, as for every migration). The key index covers live
-- rows only: a hardware key survives a reinstall, so a new install id after a
-- revoke must be able to register the same key again.
ALTER TABLE devices ADD COLUMN device_pubkey BYTEA
    CHECK (device_pubkey IS NULL OR octet_length(device_pubkey) = 33);
ALTER TABLE devices ADD COLUMN device_key_kind TEXT
    CHECK (device_key_kind IS NULL OR device_key_kind IN ('hardware', 'software'));
ALTER TABLE devices ADD CONSTRAINT devices_key_kind_with_key
    CHECK ((device_pubkey IS NULL) = (device_key_kind IS NULL));
CREATE UNIQUE INDEX devices_principal_key
    ON devices (principal_id, device_pubkey)
    WHERE device_pubkey IS NOT NULL AND revoked_at IS NULL;

-- Which agent key may be used on which device. A person's key needs no row:
-- account_key_backups already binds it to its account, and it is usable on
-- any of the account's devices with a live login session. A Kuzz agent's key
-- (principal = owner) is usable only on its host device while the owner is
-- signed in there; an external agent's only on its own paired device. Rows
-- are never re-owned; revoked rows stay for audit.
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
    ('key_registry', 'deployment-global agent key to owner account and host device binding')
ON CONFLICT (table_name) DO NOTHING;
