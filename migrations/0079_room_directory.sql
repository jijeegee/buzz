-- Room directory: which region owns each room (global regions stage S2).
--
-- `local_region` names the region this database belongs to. Each relay
-- claims it at startup from KUZZ_REGION_ID and refuses to start when the
-- database already belongs to another region. It is never copied between
-- regions.
--
-- `room_directory` has one row per room: its home region, the epoch (bumped
-- when a room moves, S6) and, for DMs, the participant hash. The unique
-- participant hash is the DM reservation: once the directory is shared
-- between regions (S5), two regions can no longer both create the DM between
-- the same people. A trigger on `channels` writes the row in the same
-- transaction as the room, so every insert path is covered. Rooms created
-- before this migration, or while no region was claimed, are filled in by
-- the relay at startup. `home_region` names a `regions` row; there is no
-- foreign key because tenant keys must lead with community_id and the
-- directory is later copied to regions in any order.
--
-- Creating the triggers locks `channels`; give up instead of queueing every
-- channel write behind a long transaction.
SET LOCAL lock_timeout = '5s';

CREATE TABLE local_region (
    singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK (singleton),
    region_id TEXT NOT NULL REFERENCES regions(id),
    claimed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('local_region', 'the region this database belongs to; never tenant data');

CREATE TABLE room_directory (
    community_id UUID NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    channel_id UUID NOT NULL,
    home_region TEXT NOT NULL CHECK (home_region ~ '^[a-z][a-z0-9-]{0,15}$'),
    epoch INTEGER NOT NULL DEFAULT 0 CHECK (epoch >= 0),
    participant_hash BYTEA,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, channel_id)
);

CREATE UNIQUE INDEX room_directory_dm_pair ON room_directory (community_id, participant_hash)
    WHERE participant_hash IS NOT NULL;
SELECT attach_community_write_fence('room_directory');

CREATE FUNCTION record_room_home() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
DECLARE
    home TEXT;
BEGIN
    SELECT region_id INTO home FROM local_region;
    IF home IS NOT NULL THEN
        INSERT INTO room_directory (community_id, channel_id, home_region, participant_hash)
        VALUES (NEW.community_id, NEW.id, home, NEW.participant_hash)
        ON CONFLICT (community_id, channel_id) DO NOTHING;
    END IF;
    RETURN NULL;
END
$$;

CREATE TRIGGER record_room_home
AFTER INSERT ON channels
FOR EACH ROW EXECUTE FUNCTION record_room_home();

CREATE FUNCTION forget_room_home() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM room_directory
     WHERE community_id = OLD.community_id AND channel_id = OLD.id;
    RETURN NULL;
END
$$;

CREATE TRIGGER forget_room_home
AFTER DELETE ON channels
FOR EACH ROW EXECUTE FUNCTION forget_room_home();
