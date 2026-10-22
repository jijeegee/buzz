-- Attachment permissions. A blob is readable only by the people who uploaded
-- it and by the people who can currently read an event that references it.
--
-- `media_uploads` records who uploaded each blob, so a sender can preview and
-- send a file before any message references it. Uploading identical bytes
-- again makes the second uploader one too: holding the bytes already reveals
-- them.
--
-- `media_references` links a blob to each event that names it (imeta `x`, any
-- `/media/<sha256>` URL in content or tags). `channel_id` NULL means the event
-- is readable by every community member (profiles, emoji sets, shared
-- personas). Ingest records a link only when the author could already open the
-- blob, so naming someone else's hash in a new event does not unlock it.
-- `event_created_at` lets the access check join the partitioned events table
-- by its primary key and skip deleted events.
CREATE TABLE media_uploads (
    community_id UUID NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    sha256       TEXT NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    uploader     BYTEA NOT NULL CHECK (length(uploader) = 32),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, sha256, uploader)
);

CREATE TABLE media_references (
    community_id     UUID NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    sha256           TEXT NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    event_id         BYTEA NOT NULL CHECK (length(event_id) = 32),
    event_created_at TIMESTAMPTZ NOT NULL,
    channel_id       UUID,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, sha256, event_id)
);

SELECT attach_community_write_fence('media_uploads');
SELECT attach_community_write_fence('media_references');

-- Backfill uploaders: whoever authored an event naming a blob before this
-- migration is treated as having uploaded it. Every kind and deleted events
-- count, so authors keep their own avatars, emoji and private-persona images
-- when a replaceable event is republished (the predecessor is deleted before
-- the new event's author check runs).
INSERT INTO media_uploads (community_id, sha256, uploader)
SELECT DISTINCT e.community_id, m.sha, e.pubkey
FROM events e
JOIN communities co ON co.id = e.community_id AND co.deletion_state = 'active'
CROSS JOIN LATERAL (
    SELECT (regexp_matches(e.content || ' ' || e.tags::text, '/media/([0-9a-f]{64})', 'g'))[1] AS sha
    UNION
    SELECT (regexp_matches(e.tags::text, '\["imeta"[^]]*"x ([0-9a-f]{64})"', 'g'))[1]
) m
WHERE length(e.pubkey) = 32
  AND (e.content LIKE '%/media/%' OR e.tags::text LIKE '%/media/%' OR e.tags::text LIKE '%"x %')
ON CONFLICT DO NOTHING;

-- Backfill links for events stored before this migration. Their authors cannot
-- be re-checked, so existing references are trusted as they are. Events no
-- other member can read are skipped (kinds inlined from kuzz_core::kind:
-- AUTHOR_ONLY_KINDS 3, 30300, 30350, 30179, 30180, 30181; owner-only 30622,
-- 44200, 24210; gift wrap 1059; personas and team catalogs 30175/30178 unless
-- shared); their authors keep access through the uploader backfill above.
INSERT INTO media_references (community_id, sha256, event_id, event_created_at, channel_id)
SELECT DISTINCT e.community_id, m.sha, e.id, e.created_at, e.channel_id
FROM events e
JOIN communities co ON co.id = e.community_id AND co.deletion_state = 'active'
CROSS JOIN LATERAL (
    SELECT (regexp_matches(e.content || ' ' || e.tags::text, '/media/([0-9a-f]{64})', 'g'))[1] AS sha
    UNION
    SELECT (regexp_matches(e.tags::text, '\["imeta"[^]]*"x ([0-9a-f]{64})"', 'g'))[1]
) m
WHERE e.deleted_at IS NULL
  AND e.kind NOT IN (3, 1059, 24210, 30179, 30180, 30181, 30300, 30350, 30622, 44200)
  AND (e.kind NOT IN (30175, 30178) OR e.tags @> '[["shared", "true"]]'::jsonb)
  AND (e.content LIKE '%/media/%' OR e.tags::text LIKE '%/media/%' OR e.tags::text LIKE '%"x %')
ON CONFLICT DO NOTHING;
