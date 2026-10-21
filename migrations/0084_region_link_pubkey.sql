-- Region link keys (global regions stage S4).
--
-- Relays of different regions call each other over the region link. Each call
-- carries an entry attestation signed with the calling region's link key; the
-- receiving region checks it against this column. NULL: the region has no
-- link (it cannot call or be called).
ALTER TABLE regions ADD COLUMN link_pubkey TEXT
    CHECK (link_pubkey ~ '^[0-9a-f]{64}$');
