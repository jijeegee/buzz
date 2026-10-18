-- ── Chats: stored latest message, thread name, and unread counts ─────────────
-- The Telegram dialog model: every room and thread keeps its latest message
-- id (top_message), and every reader keeps an unread count and an unread
-- mention count next to their read position (unread_count,
-- unread_mentions_count). The chats list reads these stored values instead of
-- scanning messages for every row.
--
-- Counting rule (the one the chats list used to apply on every read): message
-- kinds 9, 40002, 45001, 45003 after the reader's read position, not deleted,
-- not the reader's own. A room counts its main timeline (top-level messages
-- and broadcast replies); a thread counts its replies. Close and reopen
-- marker replies never count, in rooms or threads. In a DM, messages from
-- someone the reader blocked do not count. Mentions are the counted messages
-- with an event_mentions row for the reader. Both counts stop at 100 (the
-- client shows "99+").
--
-- Who keeps the counts right:
--   new message            relay write path adds 1 (crates/kuzz-db chat_state.rs)
--   thread renamed         AFTER INSERT trigger stores the newest name
--   read position moved    BEFORE trigger recounts that row
--   join / rejoin          BEFORE trigger recounts the row (and, on rejoin,
--                          the member's thread rows in that room)
--   new follow row         BEFORE trigger recounts the row
--   message deleted        AFTER trigger recounts rows that had it unread and
--                          moves a stored latest id that pointed at it
--   block / unblock        AFTER trigger recounts the blocker's DM rows
-- Every recount takes the counting rule above, capped at 100, so its cost is
-- one chats-list row of the old query.
--
-- Locks: writers that change several rows of one room take the room lock
-- (the channel-membership advisory lock the event-sequence trigger takes)
-- before any channel_members or thread_follows row lock, and update
-- channel_members before thread_follows.
--
-- The whole migration is idempotent. Rolling back to an older relay needs the
-- 0072 row removed from _sqlx_migrations (sqlx refuses unknown versions); the
-- older relay does not add 1 for new messages, so re-deploying re-runs this
-- file, which ends with the refill below. `SELECT refill_chat_stored_state()`
-- can also be run by hand at any time.

ALTER TABLE thread_state ADD COLUMN IF NOT EXISTS last_event_id BYTEA;
ALTER TABLE thread_state ADD COLUMN IF NOT EXISTS name TEXT;
ALTER TABLE channel_members ADD COLUMN IF NOT EXISTS unread_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE channel_members ADD COLUMN IF NOT EXISTS unread_mentions INTEGER NOT NULL DEFAULT 0;
ALTER TABLE thread_follows ADD COLUMN IF NOT EXISTS unread_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE thread_follows ADD COLUMN IF NOT EXISTS unread_mentions INTEGER NOT NULL DEFAULT 0;

-- A reply whose tags close or reopen its own thread (the desktop's
-- isThreadMarkerReply, matched against the reply's root).
CREATE OR REPLACE FUNCTION chat_is_thread_marker(p_tags JSONB, p_root BYTEA)
RETURNS BOOLEAN
LANGUAGE sql IMMUTABLE AS $$
    SELECT p_root IS NOT NULL AND EXISTS (
        SELECT 1 FROM jsonb_array_elements(p_tags) mt
        WHERE mt->>0 IN ('kuzz:thread-closed', 'kuzz:thread-reopened')
          AND lower(mt->>1) = encode(p_root, 'hex'))
$$;

-- Unread and unread-mention counts of a room's main timeline for one reader.
CREATE OR REPLACE FUNCTION chat_room_unread(
    p_cid UUID, p_chid UUID, p_reader BYTEA, p_after BIGINT,
    OUT unread INTEGER, OUT mentions INTEGER)
LANGUAGE plpgsql STABLE AS $$
DECLARE
    v_dm BOOLEAN;
    v_reader_hex TEXT := encode(p_reader, 'hex');
BEGIN
    SELECT c.channel_type = 'dm' INTO v_dm
      FROM channels c WHERE c.community_id = p_cid AND c.id = p_chid;
    v_dm := COALESCE(v_dm, FALSE);
    SELECT count(*) INTO unread FROM (
        SELECT 1 FROM events e
         WHERE e.community_id = p_cid AND e.channel_id = p_chid
           AND e.channel_sequence > COALESCE(p_after, 0)
           AND e.kind IN (9, 40002, 45001, 45003) AND e.deleted_at IS NULL
           AND e.pubkey <> p_reader
           AND NOT EXISTS (
               SELECT 1 FROM thread_metadata r
                WHERE r.community_id = p_cid AND r.event_id = e.id AND r.depth > 0
                  AND (NOT r.broadcast OR chat_is_thread_marker(e.tags, r.root_event_id)))
           AND NOT (v_dm AND EXISTS (
               SELECT 1 FROM user_blocks ub
                WHERE ub.community_id = p_cid AND ub.blocker = p_reader
                  AND ub.blocked = e.pubkey))
         LIMIT 100) n;
    SELECT count(*) INTO mentions FROM (
        SELECT 1 FROM events e
         WHERE e.community_id = p_cid AND e.channel_id = p_chid
           AND e.channel_sequence > COALESCE(p_after, 0)
           AND e.kind IN (9, 40002, 45001, 45003) AND e.deleted_at IS NULL
           AND e.pubkey <> p_reader
           AND NOT EXISTS (
               SELECT 1 FROM thread_metadata r
                WHERE r.community_id = p_cid AND r.event_id = e.id AND r.depth > 0
                  AND (NOT r.broadcast OR chat_is_thread_marker(e.tags, r.root_event_id)))
           AND NOT (v_dm AND EXISTS (
               SELECT 1 FROM user_blocks ub
                WHERE ub.community_id = p_cid AND ub.blocker = p_reader
                  AND ub.blocked = e.pubkey))
           AND EXISTS (
               SELECT 1 FROM event_mentions em
                WHERE em.community_id = p_cid AND em.pubkey_hex = v_reader_hex
                  AND em.event_id = e.id)
         LIMIT 100) n;
END
$$;

-- Unread and unread-mention counts of a thread for one reader. `p_chid` is
-- the thread's room (it decides whether the DM block rule applies).
CREATE OR REPLACE FUNCTION chat_thread_unread(
    p_cid UUID, p_chid UUID, p_root BYTEA, p_reader BYTEA, p_after BIGINT,
    OUT unread INTEGER, OUT mentions INTEGER)
LANGUAGE plpgsql STABLE AS $$
DECLARE
    v_dm BOOLEAN;
    v_reader_hex TEXT := encode(p_reader, 'hex');
BEGIN
    SELECT c.channel_type = 'dm' INTO v_dm
      FROM channels c WHERE c.community_id = p_cid AND c.id = p_chid;
    v_dm := COALESCE(v_dm, FALSE);
    SELECT count(*) INTO unread FROM (
        SELECT 1 FROM thread_metadata r
          JOIN events e ON e.community_id = r.community_id
           AND e.created_at = r.event_created_at AND e.id = r.event_id
         WHERE r.community_id = p_cid AND r.root_event_id = p_root AND r.depth > 0
           AND e.channel_sequence > COALESCE(p_after, 0)
           AND e.kind IN (9, 40002, 45001, 45003) AND e.deleted_at IS NULL
           AND e.pubkey <> p_reader
           AND NOT chat_is_thread_marker(e.tags, p_root)
           AND NOT (v_dm AND EXISTS (
               SELECT 1 FROM user_blocks ub
                WHERE ub.community_id = p_cid AND ub.blocker = p_reader
                  AND ub.blocked = e.pubkey))
         LIMIT 100) n;
    SELECT count(*) INTO mentions FROM (
        SELECT 1 FROM thread_metadata r
          JOIN events e ON e.community_id = r.community_id
           AND e.created_at = r.event_created_at AND e.id = r.event_id
         WHERE r.community_id = p_cid AND r.root_event_id = p_root AND r.depth > 0
           AND e.channel_sequence > COALESCE(p_after, 0)
           AND e.kind IN (9, 40002, 45001, 45003) AND e.deleted_at IS NULL
           AND e.pubkey <> p_reader
           AND NOT chat_is_thread_marker(e.tags, p_root)
           AND NOT (v_dm AND EXISTS (
               SELECT 1 FROM user_blocks ub
                WHERE ub.community_id = p_cid AND ub.blocker = p_reader
                  AND ub.blocked = e.pubkey))
           AND EXISTS (
               SELECT 1 FROM event_mentions em
                WHERE em.community_id = p_cid AND em.pubkey_hex = v_reader_hex
                  AND em.event_id = e.id)
         LIMIT 100) n;
END
$$;

-- Latest live main-timeline message of a room (not a marker).
CREATE OR REPLACE FUNCTION chat_room_latest_id(p_cid UUID, p_chid UUID)
RETURNS BYTEA
LANGUAGE sql STABLE AS $$
    SELECT e.id FROM events e
     WHERE e.community_id = p_cid AND e.channel_id = p_chid
       AND e.kind IN (9, 40002, 45001, 45003) AND e.deleted_at IS NULL
       AND NOT EXISTS (
           SELECT 1 FROM thread_metadata r
            WHERE r.community_id = p_cid AND r.event_id = e.id AND r.depth > 0
              AND (NOT r.broadcast OR chat_is_thread_marker(e.tags, r.root_event_id)))
     ORDER BY e.channel_sequence DESC LIMIT 1
$$;

-- Latest live reply of a thread (not a marker).
CREATE OR REPLACE FUNCTION chat_thread_latest_id(p_cid UUID, p_root BYTEA)
RETURNS BYTEA
LANGUAGE sql STABLE AS $$
    SELECT e.id FROM thread_metadata r
      JOIN events e ON e.community_id = r.community_id
       AND e.created_at = r.event_created_at AND e.id = r.event_id
     WHERE r.community_id = p_cid AND r.root_event_id = p_root AND r.depth > 0
       AND e.kind IN (9, 40002, 45001, 45003) AND e.deleted_at IS NULL
       AND NOT chat_is_thread_marker(e.tags, p_root)
     ORDER BY e.channel_sequence DESC LIMIT 1
$$;

-- Newest thread name (kind 40009) in the thread's room; an empty name clears.
CREATE OR REPLACE FUNCTION chat_thread_name(p_cid UUID, p_chid UUID, p_root BYTEA)
RETURNS TEXT
LANGUAGE sql STABLE AS $$
    SELECT NULLIF(e.content, '') FROM events e
     WHERE e.community_id = p_cid AND e.channel_id = p_chid AND e.kind = 40009
       AND e.deleted_at IS NULL
       AND e.tags @> jsonb_build_array(jsonb_build_array('e', encode(p_root, 'hex')))
     ORDER BY e.created_at DESC, e.id DESC LIMIT 1
$$;

-- Point a room's stored latest id at its latest live main message. With
-- `p_only_if`, only when it currently points at that event. The sort time
-- and sequence (main_last_seq / main_last_at) never move back.
CREATE OR REPLACE FUNCTION chat_set_room_latest(p_cid UUID, p_chid UUID, p_only_if BYTEA)
RETURNS VOID
LANGUAGE sql AS $$
    UPDATE channels SET main_last_event_id = chat_room_latest_id(p_cid, p_chid)
     WHERE community_id = p_cid AND id = p_chid
       AND (p_only_if IS NULL OR main_last_event_id = p_only_if)
$$;

-- Recount current members of a room, optionally only those whose read
-- position is below `p_below` and who are not `p_skip`.
CREATE OR REPLACE FUNCTION chat_recount_room_members(
    p_cid UUID, p_chid UUID, p_below BIGINT, p_skip BYTEA)
RETURNS VOID
LANGUAGE sql AS $$
    UPDATE channel_members cm
       SET (unread_count, unread_mentions) = (
           SELECT u.unread, u.mentions
             FROM chat_room_unread(cm.community_id, cm.channel_id, cm.pubkey, cm.read_seq) u)
     WHERE cm.community_id = p_cid AND cm.channel_id = p_chid AND cm.removed_at IS NULL
       AND (p_below IS NULL OR COALESCE(cm.read_seq, 0) < p_below)
       AND (p_skip IS NULL OR cm.pubkey <> p_skip)
$$;

-- Recount follow rows: of one thread (`p_root`), or of one room (`p_chid`),
-- optionally one reader's, below a read position, and not `p_skip`.
CREATE OR REPLACE FUNCTION chat_recount_thread_follows(
    p_cid UUID, p_root BYTEA, p_chid UUID, p_reader BYTEA, p_below BIGINT, p_skip BYTEA)
RETURNS VOID
LANGUAGE sql AS $$
    UPDATE thread_follows tf
       SET (unread_count, unread_mentions) = (
           SELECT u.unread, u.mentions
             FROM chat_thread_unread(tf.community_id, tf.channel_id, tf.root_event_id,
                                     tf.pubkey, tf.read_seq) u)
     WHERE tf.community_id = p_cid
       AND (p_root IS NULL OR tf.root_event_id = p_root)
       AND (p_chid IS NULL OR tf.channel_id = p_chid)
       AND (p_reader IS NULL OR tf.pubkey = p_reader)
       AND (p_below IS NULL OR COALESCE(tf.read_seq, 0) < p_below)
       AND (p_skip IS NULL OR tf.pubkey <> p_skip)
$$;

-- A read position at or past the room's last accepted sequence has nothing
-- unread: no committed message has a higher sequence (a message still being
-- written gets its +1 when it commits). The common case (read to the end,
-- or the author's own message) skips counting.
CREATE OR REPLACE FUNCTION chat_read_to_end(p_cid UUID, p_chid UUID, p_read_seq BIGINT)
RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT p_read_seq IS NOT NULL AND EXISTS (
        SELECT 1 FROM channels c
         WHERE c.community_id = p_cid AND c.id = p_chid
           AND c.accepted_sequence <= p_read_seq)
$$;

-- channel_members: recount on insert, when the read position moves, and on
-- rejoin. Runs after stamp_channel_membership_cutoff (BEFORE triggers fire in
-- name order), which holds the room lock for joins, so the count sees every
-- message committed before the join.
CREATE OR REPLACE FUNCTION tally_channel_member_unread() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.removed_at IS NOT NULL THEN
        RETURN NEW;
    END IF;
    IF TG_OP = 'UPDATE' AND NEW.read_seq IS NOT DISTINCT FROM OLD.read_seq
       AND OLD.removed_at IS NULL THEN
        RETURN NEW;
    END IF;
    IF chat_read_to_end(NEW.community_id, NEW.channel_id, NEW.read_seq) THEN
        NEW.unread_count := 0;
        NEW.unread_mentions := 0;
    ELSE
        SELECT u.unread, u.mentions INTO NEW.unread_count, NEW.unread_mentions
          FROM chat_room_unread(NEW.community_id, NEW.channel_id, NEW.pubkey, NEW.read_seq) u;
    END IF;
    IF TG_OP = 'UPDATE' AND OLD.removed_at IS NOT NULL THEN
        PERFORM chat_recount_thread_follows(
            NEW.community_id, NULL, NEW.channel_id, NEW.pubkey, NULL, NULL);
    END IF;
    RETURN NEW;
END
$$;

DROP TRIGGER IF EXISTS tally_channel_member_unread_insert ON channel_members;
CREATE TRIGGER tally_channel_member_unread_insert
BEFORE INSERT ON channel_members
FOR EACH ROW EXECUTE FUNCTION tally_channel_member_unread();

DROP TRIGGER IF EXISTS tally_channel_member_unread_update ON channel_members;
CREATE TRIGGER tally_channel_member_unread_update
BEFORE UPDATE ON channel_members
FOR EACH ROW
WHEN (OLD.read_seq IS DISTINCT FROM NEW.read_seq
      OR (OLD.removed_at IS NOT NULL AND NEW.removed_at IS NULL))
EXECUTE FUNCTION tally_channel_member_unread();

-- thread_follows: recount on insert and when the read position moves.
CREATE OR REPLACE FUNCTION tally_thread_follow_unread() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND NEW.read_seq IS NOT DISTINCT FROM OLD.read_seq THEN
        RETURN NEW;
    END IF;
    IF chat_read_to_end(NEW.community_id, NEW.channel_id, NEW.read_seq) THEN
        NEW.unread_count := 0;
        NEW.unread_mentions := 0;
    ELSE
        SELECT u.unread, u.mentions INTO NEW.unread_count, NEW.unread_mentions
          FROM chat_thread_unread(NEW.community_id, NEW.channel_id, NEW.root_event_id,
                                  NEW.pubkey, NEW.read_seq) u;
    END IF;
    RETURN NEW;
END
$$;

DROP TRIGGER IF EXISTS tally_thread_follow_unread_insert ON thread_follows;
CREATE TRIGGER tally_thread_follow_unread_insert
BEFORE INSERT ON thread_follows
FOR EACH ROW EXECUTE FUNCTION tally_thread_follow_unread();

DROP TRIGGER IF EXISTS tally_thread_follow_unread_update ON thread_follows;
CREATE TRIGGER tally_thread_follow_unread_update
BEFORE UPDATE ON thread_follows
FOR EACH ROW
WHEN (OLD.read_seq IS DISTINCT FROM NEW.read_seq)
EXECUTE FUNCTION tally_thread_follow_unread();

-- A message (or thread name) deleted or restored: recount the rows that had
-- it unread and move a stored latest id or name that pointed at it.
CREATE OR REPLACE FUNCTION chat_state_after_message_delete() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
DECLARE
    v_root BYTEA;
    v_broadcast BOOLEAN;
    v_marker BOOLEAN := FALSE;
BEGIN
    -- Same filter as the trigger's WHEN clause, kept here as well so the
    -- function stays correct if a schema tool drops the clause.
    IF OLD.deleted_at IS NOT DISTINCT FROM NEW.deleted_at OR NEW.channel_id IS NULL
       OR NEW.kind NOT IN (9, 40002, 45001, 45003, 40009) THEN
        RETURN NULL;
    END IF;
    -- Communities being deleted are write-fenced; their rows are going away.
    IF NOT EXISTS (SELECT 1 FROM communities
                    WHERE id = NEW.community_id AND deletion_state = 'active') THEN
        RETURN NULL;
    END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended(
        'kuzz_channel_membership:' || NEW.community_id::text || ':' || NEW.channel_id::text, 0));

    IF NEW.kind = 40009 THEN
        UPDATE thread_state ts
           SET name = chat_thread_name(ts.community_id, ts.channel_id, ts.root_event_id)
         WHERE ts.community_id = NEW.community_id AND ts.channel_id = NEW.channel_id
           AND ts.root_event_id IN (
               SELECT decode(t->>1, 'hex') FROM jsonb_array_elements(NEW.tags) t
                WHERE t->>0 = 'e' AND t->>1 ~ '^[0-9a-f]{64}$');
        RETURN NULL;
    END IF;

    SELECT r.root_event_id, r.broadcast INTO v_root, v_broadcast
      FROM thread_metadata r
     WHERE r.community_id = NEW.community_id AND r.event_id = NEW.id AND r.depth > 0
     LIMIT 1;
    IF v_root IS NOT NULL THEN
        v_marker := chat_is_thread_marker(NEW.tags, v_root);
    END IF;

    IF v_root IS NULL OR (v_broadcast AND NOT v_marker) THEN
        PERFORM chat_recount_room_members(
            NEW.community_id, NEW.channel_id, NEW.channel_sequence, NEW.pubkey);
        -- Deleted: only when it was the latest. Restored: it may be newer.
        PERFORM chat_set_room_latest(NEW.community_id, NEW.channel_id,
            CASE WHEN NEW.deleted_at IS NOT NULL THEN NEW.id END);
    END IF;
    IF v_root IS NOT NULL AND NOT v_marker THEN
        PERFORM chat_recount_thread_follows(
            NEW.community_id, v_root, NULL, NULL, NEW.channel_sequence, NEW.pubkey);
        UPDATE thread_state
           SET last_event_id = chat_thread_latest_id(NEW.community_id, v_root)
         WHERE community_id = NEW.community_id AND root_event_id = v_root
           AND (NEW.deleted_at IS NULL OR last_event_id = NEW.id);
    END IF;
    RETURN NULL;
END
$$;

DROP TRIGGER IF EXISTS chat_state_after_message_delete ON events;
CREATE TRIGGER chat_state_after_message_delete
AFTER UPDATE OF deleted_at ON events
FOR EACH ROW
WHEN (OLD.deleted_at IS DISTINCT FROM NEW.deleted_at
      AND NEW.channel_id IS NOT NULL
      AND NEW.kind IN (9, 40002, 45001, 45003, 40009))
EXECUTE FUNCTION chat_state_after_message_delete();

-- A thread name (kind 40009) written by any path: store the thread's newest
-- name. The insert already holds the room lock (sequence trigger).
CREATE OR REPLACE FUNCTION chat_state_after_thread_name() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    -- Same filter as the trigger's WHEN clause (see the delete trigger).
    IF NEW.kind <> 40009 OR NEW.channel_id IS NULL OR NEW.deleted_at IS NOT NULL THEN
        RETURN NULL;
    END IF;
    INSERT INTO thread_state (community_id, root_event_id, channel_id, name)
    SELECT NEW.community_id, root.id, NEW.channel_id,
           chat_thread_name(NEW.community_id, NEW.channel_id, root.id)
      FROM (SELECT DISTINCT decode(t->>1, 'hex') AS id
              FROM jsonb_array_elements(NEW.tags) t
             WHERE t->>0 = 'e' AND t->>1 ~ '^[0-9a-f]{64}$') root
     WHERE EXISTS (SELECT 1 FROM events r
                    WHERE r.community_id = NEW.community_id AND r.id = root.id
                      AND r.channel_id = NEW.channel_id)
    ON CONFLICT (community_id, root_event_id) DO UPDATE
    SET name = chat_thread_name(thread_state.community_id, thread_state.channel_id,
                                thread_state.root_event_id);
    RETURN NULL;
END
$$;

DROP TRIGGER IF EXISTS chat_state_after_thread_name ON events;
CREATE TRIGGER chat_state_after_thread_name
AFTER INSERT ON events
FOR EACH ROW
WHEN (NEW.kind = 40009 AND NEW.channel_id IS NOT NULL AND NEW.deleted_at IS NULL)
EXECUTE FUNCTION chat_state_after_thread_name();

-- A block added or removed: recount the blocker's rows in DMs the other
-- person is (or was) in. Each DM's room lock is taken first (in channel id
-- order), so a message from the other person commits entirely before the
-- recount or is written after the block change is visible.
CREATE OR REPLACE FUNCTION chat_state_after_block_change() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
DECLARE
    v_cid UUID;
    v_blocker BYTEA;
    v_blocked BYTEA;
    v_chid UUID;
BEGIN
    IF TG_OP = 'DELETE' THEN
        v_cid := OLD.community_id; v_blocker := OLD.blocker; v_blocked := OLD.blocked;
    ELSE
        v_cid := NEW.community_id; v_blocker := NEW.blocker; v_blocked := NEW.blocked;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM communities
                    WHERE id = v_cid AND deletion_state = 'active') THEN
        RETURN NULL;
    END IF;
    FOR v_chid IN
        SELECT c.id FROM channels c
         WHERE c.community_id = v_cid AND c.channel_type = 'dm'
           AND EXISTS (SELECT 1 FROM channel_members a
                        WHERE a.community_id = v_cid AND a.channel_id = c.id
                          AND a.pubkey = v_blocker)
           AND EXISTS (SELECT 1 FROM channel_members o
                        WHERE o.community_id = v_cid AND o.channel_id = c.id
                          AND o.pubkey = v_blocked)
         ORDER BY c.id
    LOOP
        PERFORM pg_advisory_xact_lock(hashtextextended(
            'kuzz_channel_membership:' || v_cid::text || ':' || v_chid::text, 0));
    END LOOP;
    UPDATE channel_members cm
       SET (unread_count, unread_mentions) = (
           SELECT u.unread, u.mentions
             FROM chat_room_unread(cm.community_id, cm.channel_id, cm.pubkey, cm.read_seq) u)
      FROM channels c
     WHERE c.community_id = cm.community_id AND c.id = cm.channel_id AND c.channel_type = 'dm'
       AND cm.community_id = v_cid AND cm.pubkey = v_blocker AND cm.removed_at IS NULL
       AND EXISTS (SELECT 1 FROM channel_members o
                    WHERE o.community_id = cm.community_id AND o.channel_id = cm.channel_id
                      AND o.pubkey = v_blocked);
    UPDATE thread_follows tf
       SET (unread_count, unread_mentions) = (
           SELECT u.unread, u.mentions
             FROM chat_thread_unread(tf.community_id, tf.channel_id, tf.root_event_id,
                                     tf.pubkey, tf.read_seq) u)
      FROM channels c
     WHERE c.community_id = tf.community_id AND c.id = tf.channel_id AND c.channel_type = 'dm'
       AND tf.community_id = v_cid AND tf.pubkey = v_blocker
       AND EXISTS (SELECT 1 FROM channel_members o
                    WHERE o.community_id = tf.community_id AND o.channel_id = tf.channel_id
                      AND o.pubkey = v_blocked);
    RETURN NULL;
END
$$;

DROP TRIGGER IF EXISTS chat_state_after_block_change ON user_blocks;
CREATE TRIGGER chat_state_after_block_change
AFTER INSERT OR DELETE ON user_blocks
FOR EACH ROW EXECUTE FUNCTION chat_state_after_block_change();

-- Fill every stored value of one room from its messages, under the room lock.
-- Also run by the chat-state backfill command for each room it visits.
CREATE OR REPLACE FUNCTION refill_chat_stored_state_channel(p_cid UUID, p_chid UUID)
RETURNS VOID
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(hashtextextended(
        'kuzz_channel_membership:' || p_cid::text || ':' || p_chid::text, 0));
    PERFORM chat_set_room_latest(p_cid, p_chid, NULL);
    -- Named threads with no state row yet (no reply so far).
    INSERT INTO thread_state (community_id, root_event_id, channel_id)
    SELECT DISTINCT p_cid, decode(t->>1, 'hex'), p_chid
      FROM events e
      CROSS JOIN LATERAL jsonb_array_elements(e.tags) t
     WHERE e.community_id = p_cid AND e.channel_id = p_chid AND e.kind = 40009
       AND e.deleted_at IS NULL AND t->>0 = 'e' AND t->>1 ~ '^[0-9a-f]{64}$'
       AND EXISTS (SELECT 1 FROM events root
                    WHERE root.community_id = p_cid AND root.id = decode(t->>1, 'hex')
                      AND root.channel_id = p_chid)
    ON CONFLICT (community_id, root_event_id) DO NOTHING;
    UPDATE thread_state ts
       SET last_event_id = chat_thread_latest_id(ts.community_id, ts.root_event_id),
           name = chat_thread_name(ts.community_id, ts.channel_id, ts.root_event_id)
     WHERE ts.community_id = p_cid AND ts.channel_id = p_chid;
    PERFORM chat_recount_room_members(p_cid, p_chid, NULL, NULL);
    PERFORM chat_recount_thread_follows(p_cid, NULL, p_chid, NULL, NULL, NULL);
END
$$;

-- Every live room of every community not being deleted. Returns the number
-- of rooms filled.
CREATE OR REPLACE FUNCTION refill_chat_stored_state() RETURNS BIGINT
LANGUAGE plpgsql AS $$
DECLARE
    room RECORD;
    filled BIGINT := 0;
BEGIN
    FOR room IN
        SELECT c.community_id AS cid, c.id AS chid
          FROM channels c
          JOIN communities co ON co.id = c.community_id AND co.deletion_state = 'active'
         WHERE c.deleted_at IS NULL
         ORDER BY c.community_id, c.id
    LOOP
        PERFORM refill_chat_stored_state_channel(room.cid, room.chid);
        filled := filled + 1;
    END LOOP;
    RETURN filled;
END
$$;

SELECT refill_chat_stored_state();
