-- since_join, second step (0081 first):
--  * a thread reply is cut together with its root, even when the reply came
--    after the reader's join (events.thread_root_sequence);
--  * open rooms follow the policy too: members get their floor, non-members
--    read no history (room state stays visible);
--  * group chats always cut, whatever their stored policy;
--  * memberships may be promoted to legacy (full history) when a room is
--    switched to since_join, so current members keep what they could see.

-- Channel sequence of the thread root of a reply in the same channel; 0 for
-- everything else. Visibility uses the smaller of the two (see
-- event_history_sequence). No CHECK: validating one would scan every events
-- partition under the table lock; only the trigger and backfill write it.
ALTER TABLE events ADD COLUMN thread_root_sequence BIGINT NOT NULL DEFAULT 0;

-- The order number that decides whether a reader sees an event: its own, or
-- its thread root's when that is earlier.
CREATE OR REPLACE FUNCTION event_history_sequence(p_seq BIGINT, p_root_seq BIGINT)
RETURNS BIGINT
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE WHEN p_root_seq > 0 AND p_root_seq < p_seq THEN p_root_seq ELSE p_seq END
$$;

-- Every reply insert path writes thread_metadata in the event's transaction.
CREATE OR REPLACE FUNCTION stamp_thread_root_sequence() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
DECLARE
    v_root BIGINT;
BEGIN
    IF NEW.depth <= 0 OR NEW.root_event_id IS NULL THEN
        RETURN NULL;
    END IF;
    SELECT r.channel_sequence INTO v_root FROM events r
     WHERE r.community_id = NEW.community_id AND r.id = NEW.root_event_id
       AND r.channel_id = NEW.channel_id
       AND (NEW.root_event_created_at IS NULL OR r.created_at = NEW.root_event_created_at)
     LIMIT 1;
    IF COALESCE(v_root, 0) > 0 THEN
        UPDATE events SET thread_root_sequence = v_root
         WHERE community_id = NEW.community_id AND id = NEW.event_id
           AND created_at = NEW.event_created_at AND thread_root_sequence <> v_root;
    END IF;
    RETURN NULL;
END
$$;

DROP TRIGGER IF EXISTS stamp_thread_root_sequence ON thread_metadata;
CREATE TRIGGER stamp_thread_root_sequence
AFTER INSERT ON thread_metadata
FOR EACH ROW EXECUTE FUNCTION stamp_thread_root_sequence();

-- Write-fenced (deleting or deleted) communities are skipped, as in 0081:
-- their rows are not served and cannot be written.
UPDATE events e SET thread_root_sequence = r.channel_sequence
  FROM thread_metadata tm
  JOIN events r ON r.community_id = tm.community_id AND r.id = tm.root_event_id
               AND (tm.root_event_created_at IS NULL OR r.created_at = tm.root_event_created_at)
  JOIN communities co ON co.id = tm.community_id AND co.deletion_state = 'active'
 WHERE tm.depth > 0 AND r.channel_id = tm.channel_id AND r.channel_sequence > 0
   AND e.community_id = tm.community_id AND e.id = tm.event_id
   AND e.created_at = tm.event_created_at;

-- True when the room cuts history at the join (since_join rooms, open or not,
-- and every group chat).
CREATE OR REPLACE FUNCTION channel_cuts_history(p_cid UUID, p_chid UUID)
RETURNS BOOLEAN
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT EXISTS (
        SELECT 1 FROM channels hc
         WHERE hc.community_id = p_cid AND hc.id = p_chid
           AND (hc.history_policy = 'since_join' OR hc.group_chat))
$$;

-- Redefines 0081. Non-members of a cutting room (open rooms included) get the
-- largest sequence: they read no history, only room state.
CREATE OR REPLACE FUNCTION channel_history_floor(p_cid UUID, p_chid UUID, p_reader BYTEA)
RETURNS BIGINT
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT CASE WHEN NOT channel_cuts_history(p_cid, p_chid) THEN 0
        ELSE COALESCE((
            SELECT CASE WHEN hm.join_cutoff_is_legacy THEN 0 ELSE hm.join_sequence END
              FROM channel_members hm
             WHERE hm.community_id = p_cid AND hm.channel_id = p_chid
               AND hm.pubkey = p_reader AND hm.removed_at IS NULL),
            9223372036854775807)
    END
$$;

-- Redefines 0081: targets are judged by their history sequence, so a
-- reaction, edit or deletion of a reply in an old thread is cut too.
CREATE OR REPLACE FUNCTION channel_event_targets_before_join(
    p_cid UUID, p_chid UUID, p_kind INTEGER, p_tags JSONB, p_reader BYTEA)
RETURNS BOOLEAN
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT CASE
        WHEN p_kind <> ALL (ARRAY[5, 7, 9005, 40003, 39005]) THEN false
        WHEN p_chid IS NOT NULL AND channel_history_floor(p_cid, p_chid, p_reader) = 0 THEN false
        -- A deletion is hidden only when everything it deletes is: it also
        -- removes visible messages from the reader's view.
        WHEN p_kind IN (5, 9005) THEN COALESCE((
            SELECT bool_and(channel_event_before_join(te.community_id, te.channel_id,
                    event_history_sequence(te.channel_sequence, te.thread_root_sequence),
                    p_reader))
              FROM jsonb_array_elements(p_tags) t
              JOIN events te ON te.community_id = p_cid
               AND te.id = CASE WHEN t->>1 ~ '^[0-9a-fA-F]{64}$' THEN decode(t->>1, 'hex') END
             WHERE t->>0 = 'e'), false)
        ELSE EXISTS (
            SELECT 1 FROM jsonb_array_elements(p_tags) t
            JOIN events te ON te.community_id = p_cid
             AND te.id = CASE WHEN t->>1 ~ '^[0-9a-fA-F]{64}$' THEN decode(t->>1, 'hex') END
            WHERE t->>0 = 'e'
              AND channel_event_before_join(te.community_id, te.channel_id,
                    event_history_sequence(te.channel_sequence, te.thread_root_sequence),
                    p_reader))
    END
$$;

-- Redefines 0081: "also send to channel" copies of replies in old threads
-- never count.
CREATE OR REPLACE FUNCTION chat_room_unread(
    p_cid UUID, p_chid UUID, p_reader BYTEA, p_after BIGINT,
    OUT unread INTEGER, OUT mentions INTEGER)
LANGUAGE plpgsql STABLE AS $$
DECLARE
    v_dm BOOLEAN;
    v_reader_hex TEXT := encode(p_reader, 'hex');
    v_after BIGINT := COALESCE(p_after, 0);
    v_floor BIGINT;
BEGIN
    v_dm := dm_is_one_to_one(p_cid, p_chid);
    -- (The fail-closed floor of a missing membership is ignored: the tally
    -- trigger counts a row before it is in the table and passes its own cut.)
    v_floor := COALESCE(NULLIF(channel_history_floor(p_cid, p_chid, p_reader),
                               9223372036854775807), 0);
    v_after := GREATEST(v_after, v_floor);
    SELECT count(*) INTO unread FROM (
        SELECT 1 FROM events e
         WHERE e.community_id = p_cid AND e.channel_id = p_chid
           AND e.channel_sequence > v_after
           AND (v_floor = 0 OR e.thread_root_sequence = 0 OR e.thread_root_sequence > v_floor)
           AND e.kind IN (9, 40002, 45001, 45003) AND e.deleted_at IS NULL
           AND e.pubkey <> p_reader
           AND NOT EXISTS (
               SELECT 1 FROM thread_metadata r
                WHERE r.community_id = p_cid AND r.event_id = e.id AND r.depth > 0
                  AND (NOT r.broadcast OR chat_is_thread_marker(e.tags, r.root_event_id)))
           AND NOT (v_dm AND EXISTS (
               SELECT 1 FROM user_block_periods bp
                WHERE bp.community_id = p_cid AND bp.blocker = p_reader
                  AND bp.blocked = e.pubkey AND bp.started_at <= e.received_at
                  AND (bp.ended_at IS NULL OR e.received_at < bp.ended_at)))
         LIMIT 100) n;
    SELECT count(*) INTO mentions FROM (
        SELECT 1 FROM events e
         WHERE e.community_id = p_cid AND e.channel_id = p_chid
           AND e.channel_sequence > v_after
           AND (v_floor = 0 OR e.thread_root_sequence = 0 OR e.thread_root_sequence > v_floor)
           AND e.kind IN (9, 40002, 45001, 45003) AND e.deleted_at IS NULL
           AND e.pubkey <> p_reader
           AND NOT EXISTS (
               SELECT 1 FROM thread_metadata r
                WHERE r.community_id = p_cid AND r.event_id = e.id AND r.depth > 0
                  AND (NOT r.broadcast OR chat_is_thread_marker(e.tags, r.root_event_id)))
           AND NOT (v_dm AND EXISTS (
               SELECT 1 FROM user_block_periods bp
                WHERE bp.community_id = p_cid AND bp.blocker = p_reader
                  AND bp.blocked = e.pubkey AND bp.started_at <= e.received_at
                  AND (bp.ended_at IS NULL OR e.received_at < bp.ended_at)))
           AND EXISTS (
               SELECT 1 FROM event_mentions em
                WHERE em.community_id = p_cid AND em.pubkey_hex = v_reader_hex
                  AND em.event_id = e.id)
         LIMIT 100) n;
END
$$;

-- Redefines 0081 (open rooms and group chats cut too).
CREATE OR REPLACE FUNCTION tally_channel_member_unread() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
DECLARE
    v_after BIGINT := NEW.read_seq;
BEGIN
    IF NEW.removed_at IS NOT NULL THEN
        RETURN NEW;
    END IF;
    IF TG_OP = 'UPDATE' AND NEW.read_seq IS NOT DISTINCT FROM OLD.read_seq
       AND OLD.removed_at IS NULL
       AND NEW.join_cutoff_is_legacy IS NOT DISTINCT FROM OLD.join_cutoff_is_legacy THEN
        RETURN NEW;
    END IF;
    IF chat_read_to_end(NEW.community_id, NEW.channel_id, NEW.read_seq) THEN
        NEW.unread_count := 0;
        NEW.unread_mentions := 0;
    ELSE
        -- Count only after this (re)join. The table row is not this
        -- membership yet, so channel_history_floor cannot see it.
        IF NOT NEW.join_cutoff_is_legacy
           AND channel_cuts_history(NEW.community_id, NEW.channel_id) THEN
            v_after := GREATEST(COALESCE(v_after, 0), NEW.join_sequence);
        END IF;
        SELECT u.unread, u.mentions INTO NEW.unread_count, NEW.unread_mentions
          FROM chat_room_unread(NEW.community_id, NEW.channel_id, NEW.pubkey, v_after) u;
    END IF;
    IF TG_OP = 'UPDATE' AND OLD.removed_at IS NOT NULL THEN
        PERFORM chat_recount_thread_follows(
            NEW.community_id, NULL, NEW.channel_id, NEW.pubkey, NULL, NULL);
    END IF;
    RETURN NEW;
END
$$;

-- Redefines 0067: an active membership may be promoted to legacy (keeps full
-- history when its room switches to since_join); nothing else moves the cut.
CREATE OR REPLACE FUNCTION stamp_channel_membership_cutoff() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE' THEN
        IF OLD.community_id IS DISTINCT FROM NEW.community_id
           OR OLD.channel_id IS DISTINCT FROM NEW.channel_id
           OR OLD.pubkey IS DISTINCT FROM NEW.pubkey THEN
            RAISE EXCEPTION 'channel membership identity is immutable'
                USING ERRCODE = 'check_violation';
        END IF;
        -- Active duplicate joins and role changes never advance the boundary.
        IF OLD.removed_at IS NULL OR NEW.removed_at IS NOT NULL THEN
            NEW.join_sequence := OLD.join_sequence;
            NEW.joined_at := OLD.joined_at;
            NEW.join_cutoff_is_legacy := OLD.join_cutoff_is_legacy
                OR (NEW.join_cutoff_is_legacy AND OLD.removed_at IS NULL
                    AND NEW.removed_at IS NULL);
            RETURN NEW;
        END IF;
    END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended(
        'kuzz_channel_membership:' || NEW.community_id::text || ':' || NEW.channel_id::text, 0));
    SELECT accepted_sequence INTO STRICT NEW.join_sequence FROM channels
    WHERE community_id = NEW.community_id AND id = NEW.channel_id;
    NEW.joined_at := clock_timestamp();
    NEW.join_cutoff_is_legacy := false;
    RETURN NEW;
END
$$;

-- Recount members whose stored counts predate this rule (open since_join
-- rooms, group chats, rooms with replies in old threads).
UPDATE channel_members cm
   SET (unread_count, unread_mentions) = (
       SELECT u.unread, u.mentions
         FROM chat_room_unread(cm.community_id, cm.channel_id, cm.pubkey, cm.read_seq) u)
  FROM channels c, communities co
 WHERE c.community_id = cm.community_id AND c.id = cm.channel_id
   AND (c.history_policy = 'since_join' OR c.group_chat)
   AND co.id = cm.community_id AND co.deletion_state = 'active'
   AND cm.removed_at IS NULL AND NOT cm.join_cutoff_is_legacy;
