-- What an external agent is ("Hermes"), as its connector named it when
-- pairing; NULL when it did not say. Only shown to the owner.
ALTER TABLE key_registry
    ADD COLUMN runtime TEXT CHECK (char_length(runtime) BETWEEN 1 AND 64);
