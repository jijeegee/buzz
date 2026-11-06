-- The profile an owner set for an external agent from any of their devices.
-- Once `profile_set_at` is set the relay stamps the agent's kind:0 from these
-- columns and refuses the kind:0 the agent signs itself. Disconnecting and
-- reconnecting the same key keep them. The ALTER needs an ACCESS EXCLUSIVE
-- lock on key_registry; fail quickly rather than queue. Nullable, no
-- defaults: a catalog change only, no table rewrite or scan.
SET LOCAL lock_timeout = '1s';

ALTER TABLE key_registry
    ADD COLUMN profile_name TEXT,
    ADD COLUMN profile_avatar_url TEXT,
    ADD COLUMN profile_about TEXT,
    ADD COLUMN profile_set_at TIMESTAMPTZ;
