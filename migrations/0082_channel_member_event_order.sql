-- Membership writes remember which change they came from, in the channel's
-- own order (`events.channel_sequence`, assigned by the server when an event
-- is stored, so client clocks do not matter).
--
-- A membership side effect that runs again later (outbox retry, a forwarded
-- write from another region) checks these first: the same event already
-- applied is a no-op, and an event older than the last change is skipped
-- instead of undoing it (re-adding someone removed afterwards, say).
--
-- Writes that come from no event (REST, workflows) record the channel's
-- current sequence and no event id, so any event stored before them counts
-- as older. NULL means no change has been recorded since this migration.
ALTER TABLE channel_members
    ADD COLUMN membership_seq BIGINT,
    ADD COLUMN membership_event_id BYTEA;
