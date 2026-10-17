-- Protocol rename: every database-side name that still said "buzz" now says
-- "kuzz". The relay and every client ship together and old data is not read
-- back, so this is a straight cut-over with no dual names.
--
--   * Session settings read by triggers: buzz.* -> kuzz.* (the relay sets the
--     kuzz.* names from this release on).
--   * Advisory lock key domains: buzz_* / buzz-* -> kuzz_* / kuzz-*. Every
--     caller (relay Rust code and these functions) moves in the same release,
--     so the two sides keep hashing the same key.
--   * Mesh status retention trigger/function and its d-tag / k-tag pattern.
--   * Push application profile buzz-ios-dogfood -> kuzz-ios-dogfood: stored
--     rows are rewritten and the CHECK constraint pinned to the new value.
--   * account_key_backups.version admits 2 (AAD domain kuzz/custodial-key-
--     backup/v1). `kuzz-admin reencrypt-key-backups` rewrites v1 rows to v2.
--
-- Every statement is idempotent (CREATE OR REPLACE, DROP ... IF EXISTS,
-- UPDATE ... WHERE old value). No table is rewritten: the only UPDATEs touch
-- the small push tables, and constraint checks scan push_gateway_installations
-- and account_key_backups, both a handful of rows per account/device.

-- Redefines the latest body from 0011_nip_rs_exact_tag_cardinality.sql.
CREATE OR REPLACE FUNCTION guard_nip_rs_hard_delete() RETURNS trigger AS $$
BEGIN
    IF current_setting('kuzz.nip_rs_hard_delete', true) IS DISTINCT FROM 'on' THEN
        RAISE EXCEPTION 'NIP-RS hard delete requires corrected writer opt-in'
            USING ERRCODE = 'check_violation';
    END IF;

    RETURN OLD;
END;
$$ LANGUAGE plpgsql;

-- Redefines the latest body from 0011_nip_rs_exact_tag_cardinality.sql.
CREATE OR REPLACE FUNCTION purge_soft_deleted_nip_rs() RETURNS trigger AS $$
BEGIN
    IF OLD.deleted_at IS NULL
       AND NEW.deleted_at IS NOT NULL
       AND NEW.kind = 30078
       AND NEW.d_tag ~ '^read-state:[0-9a-f]{32}$'
       AND (
           SELECT count(*)
           FROM jsonb_array_elements(CASE WHEN jsonb_typeof(NEW.tags) = 'array' THEN NEW.tags ELSE '[]'::jsonb END) tag
           WHERE jsonb_typeof(tag) = 'array'
             AND tag->0 = '"d"'::jsonb
       ) = 1
       AND EXISTS (
           SELECT 1
           FROM jsonb_array_elements(CASE WHEN jsonb_typeof(NEW.tags) = 'array' THEN NEW.tags ELSE '[]'::jsonb END) tag
           WHERE jsonb_typeof(tag) = 'array'
             AND jsonb_array_length(tag) >= 2
             AND jsonb_typeof(tag->1) = 'string'
             AND tag->>0 = 'd'
             AND tag->>1 = NEW.d_tag
       )
       AND (
           SELECT count(*)
           FROM jsonb_array_elements(CASE WHEN jsonb_typeof(NEW.tags) = 'array' THEN NEW.tags ELSE '[]'::jsonb END) tag
           WHERE tag = '["t", "read-state"]'::jsonb
       ) = 1 THEN
        PERFORM set_config('kuzz.nip_rs_hard_delete', 'on', true);

        DELETE FROM events
        WHERE community_id = NEW.community_id
          AND created_at = NEW.created_at
          AND id = NEW.id;

        DELETE FROM event_mentions
        WHERE community_id = NEW.community_id AND event_id = NEW.id;
    END IF;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

-- Redefines the latest body from 0021_created_at_fence_floor.sql.
CREATE OR REPLACE FUNCTION events_created_at_floor_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    floor_secs numeric := nullif(current_setting('kuzz.created_at_floor', true), '')::numeric;
BEGIN
    IF floor_secs IS NOT NULL
       AND floor_secs > 0
       AND NEW.channel_id IS NOT NULL
       AND NEW.created_at < clock_timestamp() - make_interval(secs => floor_secs)
    THEN
        RAISE EXCEPTION
            'events.created_at % is more than % s before commit time %; below the replica-fence floor',
            NEW.created_at, floor_secs, clock_timestamp()
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NULL;
END
$$;

-- Redefines the latest body from 0024_event_ttl_refresh_shared_lock.sql.
CREATE OR REPLACE FUNCTION refresh_channel_ttl_after_event_insert() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    channel_ttl INTEGER;
BEGIN
    -- Kind 9007 creates the channel and initializes its deadline itself.
    IF NEW.channel_id IS NOT NULL AND NEW.kind <> 9007 THEN
        BEGIN
            PERFORM pg_advisory_xact_lock_shared(hashtextextended(
                'kuzz_channel_ttl:' || NEW.community_id::text || ':' || NEW.channel_id::text, 0));

            SELECT ttl_seconds INTO channel_ttl
            FROM channels
            WHERE community_id = NEW.community_id AND id = NEW.channel_id;

            IF channel_ttl IS NOT NULL THEN
                UPDATE channels
                SET ttl_deadline = clock_timestamp() + make_interval(secs => ttl_seconds)
                WHERE community_id = NEW.community_id
                  AND id = NEW.channel_id
                  AND ttl_seconds IS NOT NULL
                  AND archived_at IS NULL
                  AND deleted_at IS NULL;
            END IF;
        EXCEPTION WHEN OTHERS THEN
            -- Preserve the existing best-effort contract: a TTL refresh failure
            -- must not reject an otherwise valid durable event.
            RAISE WARNING 'channel TTL refresh failed for community %, channel %: %',
                NEW.community_id, NEW.channel_id, SQLERRM;
        END;
    END IF;
    RETURN NULL;
END
$$;

-- Redefines the latest body from 0029_community_deletion.sql.
CREATE OR REPLACE FUNCTION community_deletion_lock_key(target UUID) RETURNS BIGINT
LANGUAGE SQL IMMUTABLE STRICT PARALLEL SAFE AS $$
    SELECT hashtextextended('kuzz-community-deletion:' || target::text, 0)
$$;

-- Redefines the latest body from 0029_community_deletion.sql.
CREATE OR REPLACE FUNCTION assert_community_write_allowed(target UUID) RETURNS VOID
LANGUAGE plpgsql AS $$
DECLARE
    lifecycle TEXT;
    generation BIGINT;
    executor_community TEXT;
    executor_generation TEXT;
    serving_community TEXT;
    serving_lease_id TEXT;
    serving_owner TEXT;
    serving_generation TEXT;
    serving_fence_generation TEXT;
    serving_lease_valid BOOLEAN := false;
BEGIN
    -- The fence proof depends on a fresh statement snapshot after the shared
    -- advisory lock is granted. Pinned RR/Serializable snapshots can retain a
    -- pre-fence lifecycle or executor generation and resurrect tenant data.
    IF current_setting('transaction_isolation') <> 'read committed' THEN
        RAISE EXCEPTION 'community writes require READ COMMITTED isolation'
            USING ERRCODE = 'invalid_transaction_state';
    END IF;

    -- Nullable operator-attribution rows without a tenant are unrelated.
    IF target IS NULL THEN
        RETURN;
    END IF;

    PERFORM pg_advisory_xact_lock_shared(community_deletion_lock_key(target));
    SELECT deletion_state, deletion_fence_generation
      INTO lifecycle, generation
      FROM communities
     WHERE id = target;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'community write rejected: community % is missing', target
            USING ERRCODE = 'object_not_in_prerequisite_state';
    END IF;

    -- Authorization is evaluated independently for every community checked.
    executor_community := current_setting('kuzz.deletion_executor_community', true);
    executor_generation := current_setting('kuzz.deletion_fence_generation', true);
    IF executor_community = target::TEXT
       AND executor_generation ~ '^[0-9]+$'
       AND executor_generation::BIGINT = generation THEN
        RETURN;
    END IF;

    -- A serving mutation admitted before quiescing may finish only while its
    -- exact durable lease remains current and bound to this fence generation.
    serving_community := current_setting('kuzz.serving_write_community', true);
    serving_lease_id := current_setting('kuzz.serving_write_lease_id', true);
    serving_owner := current_setting('kuzz.serving_write_owner', true);
    serving_generation := current_setting('kuzz.serving_write_generation', true);
    serving_fence_generation := current_setting('kuzz.serving_write_fence_generation', true);
    IF lifecycle IN ('active', 'quiescing')
       AND serving_community = target::TEXT
       AND serving_lease_id ~ '^[0-9a-fA-F-]{36}$'
       AND serving_generation ~ '^[0-9]+$'
       AND serving_fence_generation ~ '^[0-9]+$'
       AND serving_fence_generation::BIGINT = generation THEN
        SELECT EXISTS(
            SELECT 1 FROM community_serving_write_leases lease
             WHERE lease.id = serving_lease_id::UUID
               AND lease.community_id = target
               AND lease.owner = serving_owner
               AND lease.generation = serving_generation::BIGINT
               AND lease.fence_generation = serving_fence_generation::BIGINT
               AND lease.lease_until >= now()
        ) INTO serving_lease_valid;
        IF serving_lease_valid THEN
            RETURN;
        END IF;
    END IF;

    IF lifecycle <> 'active' THEN
        RAISE EXCEPTION 'community write fenced: community % generation %', target, generation
            USING ERRCODE = 'object_not_in_prerequisite_state';
    END IF;
END
$$;

-- Redefines the latest body from 0029_community_deletion.sql.
CREATE OR REPLACE FUNCTION enforce_community_tombstone() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
DECLARE
    executor_community TEXT := current_setting('kuzz.deletion_executor_community', true);
    executor_generation TEXT := current_setting('kuzz.deletion_fence_generation', true);
    expected_generation BIGINT;
BEGIN
    IF TG_OP = 'DELETE' THEN
        IF OLD.deletion_state <> 'active' OR OLD.deleted_at IS NOT NULL THEN
            RAISE EXCEPTION 'community tombstones are permanent'
                USING ERRCODE = 'object_not_in_prerequisite_state';
        END IF;
        RETURN OLD;
    END IF;

    expected_generation := CASE
        WHEN NEW.deletion_fence_generation > OLD.deletion_fence_generation
            THEN NEW.deletion_fence_generation
        ELSE OLD.deletion_fence_generation
    END;
    IF executor_community = OLD.id::text
       AND executor_generation ~ '^[0-9]+$'
       AND executor_generation::BIGINT = expected_generation THEN
        RETURN NEW;
    END IF;

    IF OLD.deletion_state <> 'active'
       OR NEW.deletion_state <> OLD.deletion_state
       OR NEW.deletion_fence_generation <> OLD.deletion_fence_generation
       OR NEW.deleted_at IS DISTINCT FROM OLD.deleted_at THEN
        RAISE EXCEPTION 'community tombstone mutation rejected: community % generation %',
            OLD.id, OLD.deletion_fence_generation
            USING ERRCODE = 'object_not_in_prerequisite_state';
    END IF;
    RETURN NEW;
END
$$;

-- Redefines the latest body from 0032_channel_roster_snapshot_fence.sql.
CREATE OR REPLACE FUNCTION guard_channel_roster_snapshot()
RETURNS TRIGGER AS $$
DECLARE
    canonical_members TEXT[];
    snapshot_members TEXT[];
BEGIN
    IF NEW.kind <> 39002 OR NEW.channel_id IS NULL THEN
        RETURN NEW;
    END IF;

    PERFORM pg_advisory_xact_lock(hashtextextended(
        'kuzz_channel_membership:' || NEW.community_id::text || ':' || NEW.channel_id::text,
        0
    ));

    SELECT COALESCE(
               array_agg(encode(cm.pubkey, 'hex') || ':' || cm.role::text ORDER BY cm.pubkey),
               ARRAY[]::TEXT[]
           )
      INTO canonical_members
      FROM channel_members cm
     WHERE cm.community_id = NEW.community_id
       AND cm.channel_id = NEW.channel_id
       AND cm.removed_at IS NULL;

    -- A roster is canonical only when every p tag uses the emitted four-field
    -- shape, contains a 32-byte hex pubkey and valid authoritative role, has no
    -- duplicate members, and exactly matches the active membership rows.
    IF EXISTS (
        SELECT 1
          FROM jsonb_array_elements(NEW.tags) AS roster_tag(tag_json)
         WHERE roster_tag.tag_json->>0 = 'p'
           AND (
               jsonb_array_length(roster_tag.tag_json) <> 4
               OR COALESCE(roster_tag.tag_json->>1, '') !~ '^[0-9a-fA-F]{64}$'
               OR roster_tag.tag_json->>2 <> ''
               OR COALESCE(roster_tag.tag_json->>3, '') NOT IN ('owner', 'admin', 'bot', 'member', 'guest')
           )
    ) THEN
        RAISE EXCEPTION 'kind 39002 roster contains an invalid p tag'
            USING ERRCODE = '23514';
    END IF;

    SELECT COALESCE(
               array_agg(
                   lower((roster_tag.tag_json->>1)) || ':' || (roster_tag.tag_json->>3)
                   ORDER BY decode((roster_tag.tag_json->>1), 'hex')
               ),
               ARRAY[]::TEXT[]
           )
      INTO snapshot_members
      FROM jsonb_array_elements(NEW.tags) AS roster_tag(tag_json)
     WHERE roster_tag.tag_json->>0 = 'p';

    IF snapshot_members IS DISTINCT FROM canonical_members THEN
        RAISE EXCEPTION 'kind 39002 roster does not match canonical channel membership'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Redefines the latest body from 0040_push_message_kinds.sql.
CREATE OR REPLACE FUNCTION enqueue_push_match_job() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    -- Keep this allowlist identical to the relay's validated NIP-PL descriptor.
    IF NEW.kind IN (9, 40002, 45001, 45003) THEN
        PERFORM pg_advisory_xact_lock_shared(
            hashtextextended('kuzz_push_gate:' || NEW.community_id::text, 0));
        IF EXISTS (
            SELECT 1 FROM push_leases
            WHERE community_id = NEW.community_id
              AND active
              AND endpoint_enabled
              AND expires_at > EXTRACT(EPOCH FROM now())::bigint
        ) THEN
            INSERT INTO push_match_queue (community_id, event_id)
            VALUES (NEW.community_id, NEW.id)
            ON CONFLICT DO NOTHING;
        END IF;
    END IF;
    RETURN NEW;
END
$$;

-- Redefines the latest body from 0060_channel_history_order.sql.
CREATE OR REPLACE FUNCTION stamp_channel_event_sequence() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.channel_id IS NULL THEN
        NEW.channel_sequence := 0;
        RETURN NEW;
    END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended(
        'kuzz_channel_membership:' || NEW.community_id::text || ':' || NEW.channel_id::text, 0));
    UPDATE channels SET accepted_sequence = accepted_sequence + 1
    WHERE community_id = NEW.community_id AND id = NEW.channel_id
    RETURNING accepted_sequence INTO NEW.channel_sequence;
    -- Existing event storage permits references to absent channels. They have
    -- no membership and cannot pass a reader's channel authorization.
    IF NOT FOUND THEN NEW.channel_sequence := 0; END IF;
    RETURN NEW;
END
$$;

-- Redefines the latest body from 0060_channel_history_order.sql.
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
            NEW.join_cutoff_is_legacy := OLD.join_cutoff_is_legacy;
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

-- Mesh status retention (0019) under the kuzz names.
DROP TRIGGER IF EXISTS trg_events_purge_soft_deleted_buzz_mesh_status ON events;
DROP FUNCTION IF EXISTS purge_soft_deleted_buzz_mesh_status();

CREATE OR REPLACE FUNCTION purge_soft_deleted_kuzz_mesh_status() RETURNS trigger AS $$
BEGIN
    IF OLD.deleted_at IS NULL
       AND NEW.deleted_at IS NOT NULL
       AND NEW.kind = 30003
       AND NEW.d_tag LIKE 'kuzz-mesh-member-status:%'
       AND NEW.tags @> '[["k", "kuzz-mesh-status"]]'::jsonb THEN
        DELETE FROM events
        WHERE community_id = NEW.community_id
          AND created_at = NEW.created_at
          AND id = NEW.id;

        DELETE FROM event_mentions
        WHERE community_id = NEW.community_id AND event_id = NEW.id;
    END IF;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_events_purge_soft_deleted_kuzz_mesh_status ON events;
CREATE TRIGGER trg_events_purge_soft_deleted_kuzz_mesh_status
    AFTER UPDATE OF deleted_at ON events
    FOR EACH ROW EXECUTE FUNCTION purge_soft_deleted_kuzz_mesh_status();

-- Push application profile (0043). Drop the pin first so the rows can move.
ALTER TABLE push_gateway_installations
    DROP CONSTRAINT IF EXISTS push_gateway_installations_app_profile_check;
UPDATE push_gateway_installations
SET app_profile = 'kuzz-ios-dogfood'
WHERE app_profile = 'buzz-ios-dogfood';
ALTER TABLE push_gateway_installations
    ADD CONSTRAINT push_gateway_installations_app_profile_check
    CHECK (app_profile = 'kuzz-ios-dogfood');

UPDATE push_leases
SET app_profile = 'kuzz-ios-dogfood'
WHERE app_profile = 'buzz-ios-dogfood';

-- Custodial key backups (0059): version 2 uses the kuzz AAD domain. Version 1
-- rows stay admissible only until `kuzz-admin reencrypt-key-backups` rewrites
-- them; the relay itself reads and writes version 2 only.
ALTER TABLE account_key_backups
    DROP CONSTRAINT IF EXISTS account_key_backups_version_check;
ALTER TABLE account_key_backups
    ADD CONSTRAINT account_key_backups_version_check CHECK (version IN (1, 2));
