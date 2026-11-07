-- Relay-wide runtime settings an operator changes without a restart
-- (`kuzz-admin settings set <key> <value>`). Relays read a key through a
-- short cache (at most 30 s). First key: `window_check_mode` (off | shadow |
-- on); a missing or invalid value means `off`.
--
-- Additive only.
CREATE TABLE relay_runtime_settings (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('relay_runtime_settings', 'deployment-global relay runtime settings set by operators')
ON CONFLICT (table_name) DO NOTHING;
