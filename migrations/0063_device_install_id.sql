-- A device is one app install, not one login. Clients send a random install
-- id they keep for the life of the install; a login with an install id the
-- principal already used reuses that device row (its id, name and every
-- per-device record keyed by it) instead of creating another. Logins without
-- one (older clients) still create a new device each time.
ALTER TABLE devices ADD COLUMN install_id TEXT;
CREATE UNIQUE INDEX devices_principal_install
    ON devices (principal_id, install_id)
    WHERE install_id IS NOT NULL;
