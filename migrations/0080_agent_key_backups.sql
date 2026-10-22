-- Kuzz agent key copies (device-bound keys plan §4, PR A3). An agent's
-- secret key is stored encrypted under the same server master key and
-- AES-GCM scheme as account_key_backups, with its own AAD domain
-- (kuzz/agent-key-backup/v1, binding owner account, agent key and master key
-- label). A copy belongs to its key_registry row: deleting the row deletes
-- the copy. host_device_id is the device the copy was registered from; the
-- registry row stays the authority on where the agent may run.
--
-- Additive only, plus the registry backfill below.
CREATE TABLE agent_key_backups (
    agent_pubkey   BYTEA PRIMARY KEY REFERENCES key_registry(pubkey) ON DELETE CASCADE,
    owner_id       BYTEA NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
    host_device_id UUID REFERENCES devices(id) ON DELETE SET NULL,
    version        SMALLINT NOT NULL CHECK (version = 1),
    key_id         TEXT NOT NULL CHECK (key_id ~ '^[A-Za-z0-9_-]{1,64}$'),
    nonce          BYTEA NOT NULL CHECK (length(nonce) = 12),
    ciphertext     BYTEA NOT NULL CHECK (length(ciphertext) = 48),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX agent_key_backups_owner_host ON agent_key_backups (owner_id, host_device_id);

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('agent_key_backups', 'deployment-global encrypted Kuzz agent key copies of owner accounts')
ON CONFLICT (table_name) DO NOTHING;

-- S1 backfill: every agent already attested to a key-backup account
-- (users.agent_owner_pubkey = the account's own key) is bound to that
-- account before any client registers, so a stolen agent key cannot be
-- claimed by another account. No host device is known yet: host NULL means
-- "unbound", refused by key admission until the owner registers it from a
-- device. An agent whose profiles name more than one owner across
-- communities is skipped: it must be registered explicitly, and
-- registration refuses an owner other than the one every profile names. A
-- key that is itself an account key is skipped. Idempotent.
INSERT INTO key_registry (pubkey, principal_id, role)
SELECT u.pubkey, b.account_id, 'agent'
  FROM (SELECT pubkey, (array_agg(agent_owner_pubkey))[1] AS owner_pubkey
          FROM users
         WHERE agent_owner_pubkey IS NOT NULL
         GROUP BY pubkey
        HAVING count(DISTINCT agent_owner_pubkey) = 1) u
  JOIN account_key_backups b ON b.pubkey = u.owner_pubkey
 WHERE u.pubkey <> u.owner_pubkey
   AND NOT EXISTS (SELECT 1 FROM account_key_backups p WHERE p.pubkey = u.pubkey)
ON CONFLICT (pubkey) DO NOTHING;
