-- Agent host devices (kind:30180) and device robot choices (kind:30181) are
-- author-only owner data. Exclude them from full-text search so search never
-- spends its budget on rows the relay must then hide. Same wrap-the-existing-
-- expression shape as 0033 (kind:30179); every other kind keeps whatever
-- search policy the database had before.
--
-- Operational cost: as with 0014/0033, DROP COLUMN + ADD ... GENERATED ...
-- STORED rewrites the events heap and rebuilds the GIN index under an ACCESS
-- EXCLUSIVE lock. Expect relay downtime proportional to the size of events.
DO $$
DECLARE
    existing_expression TEXT;
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

    ALTER TABLE events DROP COLUMN search_tsv;
    EXECUTE format(
        'ALTER TABLE events ADD COLUMN search_tsv TSVECTOR GENERATED ALWAYS AS (CASE WHEN kind IN (30180, 30181) THEN NULL::tsvector ELSE (%s) END) STORED',
        existing_expression
    );
    CREATE INDEX idx_events_search_tsv ON events USING GIN (search_tsv);
END $$;
