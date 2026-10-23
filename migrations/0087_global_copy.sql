-- Global copy (global regions stage S5-1).
--
-- Global data (accounts, communities, members, profiles, the room directory,
-- regions, room-less events) is written only in the primary region and
-- copied to every other region by Postgres logical replication (publication
-- and subscription `kuzz_global`). The relay creates the publication in the
-- primary and the write guards in a copy at startup, from one table list
-- (`kuzz_db::global_copy`), so a single-region deployment gets neither.
--
-- `local_region.global_copy` marks a database whose global tables are a
-- copy. Nothing but replication may write them there: one local row that
-- conflicts with a copied one stops replication for the whole region. The
-- guard trigger refuses such writes; the replication worker runs with
-- `session_replication_role = replica`, so ordinary triggers (the guard
-- included) do not fire for it.
--
-- A migration that writes rows into a copied table must skip copies
-- (`WHERE NOT kuzz_is_global_copy()`); the guard makes a forgotten one fail
-- instead of splitting data.
--
-- `global_heartbeat` is copied too: the primary updates it every second and
-- each region measures how far its copy is behind.
SET LOCAL lock_timeout = '5s';

ALTER TABLE local_region ADD COLUMN global_copy BOOLEAN NOT NULL DEFAULT false;

CREATE FUNCTION kuzz_is_global_copy() RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT COALESCE((SELECT global_copy FROM local_region), false)
$$;

CREATE FUNCTION refuse_global_copy_write() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'global table % is written in the primary region; this database is a copy',
        TG_TABLE_NAME
        USING ERRCODE = 'read_only_sql_transaction';
END
$$;

CREATE TABLE global_heartbeat (
    singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK (singleton),
    beat_at TIMESTAMPTZ NOT NULL
);

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('global_heartbeat', 'the primary region''s copy heartbeat; never tenant data');

-- Deleting a community also drops its undelivered region calls (S4-5 left
-- region_outbox out of the deletion fence).
SELECT attach_community_write_fence('region_outbox');

-- A copy learns rooms from the primary's directory: creating or deleting a
-- room there must not write the copied directory.
CREATE OR REPLACE FUNCTION record_room_home() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
DECLARE
    home TEXT;
BEGIN
    SELECT region_id INTO home FROM local_region WHERE NOT global_copy;
    IF home IS NOT NULL THEN
        INSERT INTO room_directory (community_id, channel_id, home_region, participant_hash)
        VALUES (NEW.community_id, NEW.id, home, NEW.participant_hash)
        ON CONFLICT (community_id, channel_id) DO NOTHING;
    END IF;
    RETURN NULL;
END
$$;

CREATE OR REPLACE FUNCTION forget_room_home() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF NOT kuzz_is_global_copy() THEN
        DELETE FROM room_directory
         WHERE community_id = OLD.community_id AND channel_id = OLD.id;
    END IF;
    RETURN NULL;
END
$$;
