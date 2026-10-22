-- Account language (language packs plan §7). The language the user picked in
-- the app, as a BCP 47 tag the apps use (en, ko, zh-Hant, pt-BR, fil). The
-- server writes text such as moderation notices in it; NULL means not chosen
-- and the server falls back to English. Stored on the account, not the region,
-- so a second region writes the same language.
--
-- Additive only.
ALTER TABLE principals
    ADD COLUMN locale TEXT
    CONSTRAINT principals_locale CHECK (locale ~ '^[a-z]{2,3}(-[A-Za-z0-9]{2,8}){0,2}$');
