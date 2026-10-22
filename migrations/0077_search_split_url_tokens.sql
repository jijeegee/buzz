-- Make words inside URLs and paths searchable.
--
-- The 'simple' parser keeps `https://x.org/auth/oidc/google/callback` or
-- `/auth/oidc/google/callback` as single url/host/file tokens, so a search for
-- `callback` (full-text or prefix `callback:*`) never matched a message whose
-- only `callback` was inside a link. Index the content twice: the original
-- tokens (exact URL/path searches keep working) plus the content with URL and
-- path punctuation turned into spaces (the inner words become lexemes).
--
-- Same capture-and-rewrite shape as 0014/0033/0062: PostgreSQL cannot alter a
-- generated expression in place, so read the current expression, swap its
-- `to_tsvector('simple', content)` term for the split form, drop the column
-- and re-add it. Every kind filter the database already has (positive
-- allowlist on fresh installs, the negative skip-sets on older ones) is kept
-- unchanged.
--
-- Operational cost: DROP COLUMN + ADD ... GENERATED ... STORED rewrites every
-- events partition and rebuilds the GIN index under an ACCESS EXCLUSIVE lock
-- inside the migration transaction. Expect relay downtime proportional to the
-- size of events.
DO $$
DECLARE
    existing_expression TEXT;
    plain_term CONSTANT TEXT := 'to_tsvector(''simple''::regconfig, content)';
    split_term CONSTANT TEXT :=
        '(to_tsvector(''simple''::regconfig, content) || '
        'to_tsvector(''simple''::regconfig, '
        'regexp_replace(content, ''[-/:._?=&#]+'', '' '', ''g'')))';
BEGIN
    SELECT pg_get_expr(d.adbin, d.adrelid)
      INTO existing_expression
      FROM pg_attrdef d
      JOIN pg_attribute a
        ON a.attrelid = d.adrelid
       AND a.attnum = d.adnum
     WHERE d.adrelid = 'events'::regclass
       AND a.attname = 'search_tsv';

    IF existing_expression IS NULL THEN
        RAISE EXCEPTION 'events.search_tsv generated expression not found';
    END IF;

    IF position(plain_term IN existing_expression) = 0 THEN
        RAISE EXCEPTION 'events.search_tsv expression has no % term: %',
            plain_term, existing_expression;
    END IF;

    ALTER TABLE events DROP COLUMN search_tsv;
    EXECUTE format(
        'ALTER TABLE events ADD COLUMN search_tsv TSVECTOR GENERATED ALWAYS AS (%s) STORED',
        replace(existing_expression, plain_term, split_term)
    );
    CREATE INDEX idx_events_search_tsv ON events USING GIN (search_tsv);
END $$;
