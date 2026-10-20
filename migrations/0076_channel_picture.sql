-- Group photo: a `/media/<sha256>[.ext]` URL on this relay, or NULL for the
-- letter avatar. Set and cleared through kind:9002 `picture` tags.
--
-- Additive only: the previous relay image ignores this column.
ALTER TABLE channels ADD COLUMN picture TEXT;
