-- Sign in with Apple: a second external identity provider. Same table and
-- rules as Google; the subject is Apple's stable `sub`. The constraint swap
-- needs an ACCESS EXCLUSIVE lock; fail quickly rather than queue.
SET LOCAL lock_timeout = '5s';

ALTER TABLE identities
    DROP CONSTRAINT identities_provider_check,
    ADD CONSTRAINT identities_provider_check CHECK (provider IN ('google', 'apple'));
