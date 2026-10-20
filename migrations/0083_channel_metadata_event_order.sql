-- Channel metadata edits (kind 9002) remember, per field, which change they
-- came from in the channel's own order (`events.channel_sequence`). A 9002
-- sets only the fields it carries, so the order is per field: an old edit of
-- the name that runs again later must not undo a newer name, but may still
-- set a topic nobody changed since.
--
-- Writes without an event (huddle and reaper archives) record the channel's
-- current `accepted_sequence` and no event id.
CREATE TABLE channel_metadata_order (
    community_id UUID NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    channel_id UUID NOT NULL,
    field TEXT NOT NULL,
    seq BIGINT NOT NULL,
    event_id BYTEA,
    PRIMARY KEY (community_id, channel_id, field),
    FOREIGN KEY (community_id, channel_id)
        REFERENCES channels (community_id, id) ON DELETE CASCADE
);
SELECT attach_community_write_fence('channel_metadata_order');
