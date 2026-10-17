-- Protocol rename: the dogfood application profile is now kuzz-ios-dogfood.
-- Rewrite stored rows and pin the constraint to the new value. Idempotent.
-- Installations registered before the rename carry token ciphertexts and
-- endpoint fingerprints bound to the old domain names, so devices register
-- again after the cut-over; the rewrite keeps the rows (and their revocation
-- tombstones) valid under the new constraint.
ALTER TABLE push_gateway_installations
    DROP CONSTRAINT IF EXISTS push_gateway_installations_app_profile_check;
UPDATE push_gateway_installations
SET app_profile = 'kuzz-ios-dogfood'
WHERE app_profile = 'buzz-ios-dogfood';
ALTER TABLE push_gateway_installations
    ADD CONSTRAINT push_gateway_installations_app_profile_check
    CHECK (app_profile = 'kuzz-ios-dogfood');
