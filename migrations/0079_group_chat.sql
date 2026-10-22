-- Group chat: an ownerless private stream (no owner/admin, no handle, no
-- invite link, every member equal). Set only at creation by a kind:9007 with
-- a `["group_chat"]` tag; never changed afterwards.
--
-- Additive only: the previous relay image ignores this column.
ALTER TABLE channels ADD COLUMN group_chat BOOLEAN NOT NULL DEFAULT false;
