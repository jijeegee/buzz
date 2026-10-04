-- Centralized identity (Phase 0): events published over a token-authenticated
-- connection are stamped by the server and carry no client signature.
--
-- Phase 0 still writes a 64-byte all-zero sentinel for those events (the
-- in-memory nostr::Event always has a signature), so no NULL is written yet.
-- The column becomes nullable now so readers are proven NULL-tolerant before
-- Phase 3 stops writing signatures and Phase 4 drops the column.
ALTER TABLE events ALTER COLUMN sig DROP NOT NULL;
