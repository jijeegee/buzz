-- Block change order across regions (global regions stage S4-5b).
--
-- With the region link on, a block or unblock is saved in the region where it
-- is made and sent to every other region. Each change carries a version (the
-- entry region's time in microseconds); a region keeps the newest version it
-- applied per pair and ignores older ones, so a block and an unblock that
-- arrive in the wrong order cannot leave regions disagreeing. The row stays
-- after an unblock (it is the tombstone).
CREATE TABLE user_block_versions (
    community_id UUID NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    blocker      BYTEA NOT NULL CHECK (length(blocker) = 32),
    blocked      BYTEA NOT NULL CHECK (length(blocked) = 32),
    version      BIGINT NOT NULL,
    PRIMARY KEY (community_id, blocker, blocked)
);

SELECT attach_community_write_fence('user_block_versions');
