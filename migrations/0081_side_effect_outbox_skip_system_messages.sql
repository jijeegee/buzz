-- System messages (kind 40099) are what the relay's side effects emit; they
-- have no side effects of their own. 0077 still queued an outbox row for
-- each one, and nothing ever settled those rows. Stop queueing them and drop
-- the rows already left.
CREATE OR REPLACE FUNCTION enqueue_event_side_effect() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.kind IN (0, 5, 10100, 30617)
       OR NEW.kind BETWEEN 9000 AND 9022
       OR NEW.kind BETWEEN 41001 AND 41003 THEN
        INSERT INTO event_side_effects (community_id, event_id, kind)
        VALUES (NEW.community_id, NEW.id, NEW.kind)
        ON CONFLICT DO NOTHING;
    END IF;
    RETURN NULL;
END
$$;

DELETE FROM event_side_effects WHERE kind = 40099;
