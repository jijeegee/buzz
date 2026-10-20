-- Each account's home region and sign-up country (global regions stage S2).
--
-- `home_region` is the region that took the sign-up. New rooms and DMs
-- default to it, and it decides which address the app is told to connect to
-- (S3). Only an operator changes it. A trigger fills it from `local_region`
-- on insert, so every way a principal is created (user, bot, relay) is
-- covered. Accounts from before this migration are filled in by the relay at
-- startup.
--
-- `country` is the two-letter country Cloudflare reports when the account
-- signs up, read only from a trusted proxy. NULL when unknown.
--
-- The ALTERs lock `principals`; give up instead of queueing logins behind a
-- long transaction.
SET LOCAL lock_timeout = '5s';

ALTER TABLE principals
    ADD COLUMN home_region TEXT CONSTRAINT principals_home_region
        CHECK (home_region ~ '^[a-z][a-z0-9-]{0,15}$');
ALTER TABLE principals
    ADD COLUMN country TEXT CONSTRAINT principals_country CHECK (country ~ '^[A-Z]{2}$');

CREATE FUNCTION set_principal_home() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.home_region IS NULL THEN
        SELECT region_id INTO NEW.home_region FROM local_region;
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER set_principal_home
BEFORE INSERT ON principals
FOR EACH ROW EXECUTE FUNCTION set_principal_home();
