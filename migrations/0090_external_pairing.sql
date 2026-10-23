-- External agent pairing (device-bound keys plan §5 and §14.7, PR A4). An
-- owner's app starts a pairing and shows `kuzz pair <code>`; an agent on
-- another computer claims the code with its key and that computer's device
-- key. The first valid claim adds the computer as an `external` device of
-- the owner (holding its device key, no login, no sessions) and binds the
-- agent key to it (key_registry role `external`); the owner disconnects it
-- by revoking that device.
--
-- Login and external rows of one account are separate: a login never
-- reuses, un-revokes or re-keys an external row, so the device key index is
-- per kind. The previous relay image knows no kinds; roll it back only
-- before the first pairing (KUZZ_AUTO_MIGRATE off, as for every migration).
ALTER TABLE devices ADD COLUMN kind TEXT NOT NULL DEFAULT 'login'
    CHECK (kind IN ('login', 'external'));
ALTER TABLE devices DROP CONSTRAINT devices_platform_check;
ALTER TABLE devices ADD CONSTRAINT devices_platform_check
    CHECK (platform IN ('desktop', 'mobile', 'web', 'cli', 'external'));
-- An external device is always keyed, never an install, and only it has
-- the `external` platform.
ALTER TABLE devices ADD CONSTRAINT devices_external_shape
    CHECK ((kind = 'external') = (platform = 'external')
           AND (kind = 'login' OR (device_pubkey IS NOT NULL AND install_id IS NULL)));
DROP INDEX devices_principal_key;
CREATE UNIQUE INDEX devices_principal_key
    ON devices (principal_id, kind, device_pubkey)
    WHERE device_pubkey IS NOT NULL AND revoked_at IS NULL;

-- One pairing: `open` (code shown, unused) -> `connected` (the first valid
-- claim added the computer and bound the agent, all at once). There is no
-- approval step: the owner's signed-in device started it and the code is
-- single use, so a later claim of the same code is refused. The code (128
-- random bits) is stored only as its SHA-256; `expires_at` is the claim
-- deadline.
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
    ('pairing_requests', 'deployment-global external agent pairing requests of owner accounts')
ON CONFLICT (table_name) DO NOTHING;
