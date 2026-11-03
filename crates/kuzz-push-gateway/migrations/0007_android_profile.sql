-- Admit the kuzz-android application profile next to kuzz-ios-dogfood.
-- Existing iOS rows are untouched. Android installations reuse the
-- installation-key columns rather than adding nullable ones:
--   app_attest_key_id     = SHA-256 of the attested key's SubjectPublicKeyInfo DER
--   app_attest_public_key = that SubjectPublicKeyInfo DER (P-256, 91 bytes)
--   assertion_counter     = gateway-side count of accepted signed mutations
-- so every NOT NULL, length and active-only uniqueness rule keeps applying
-- to both profiles. Idempotent.
ALTER TABLE push_gateway_installations
    DROP CONSTRAINT IF EXISTS push_gateway_installations_app_profile_check;
ALTER TABLE push_gateway_installations
    ADD CONSTRAINT push_gateway_installations_app_profile_check
    CHECK (app_profile IN ('kuzz-ios-dogfood', 'kuzz-android'));
COMMENT ON COLUMN push_gateway_installations.app_attest_key_id IS
    'kuzz-ios-dogfood: App Attest key id. kuzz-android: SHA-256 of the Key Attestation leaf SubjectPublicKeyInfo DER.';
COMMENT ON COLUMN push_gateway_installations.app_attest_public_key IS
    'kuzz-ios-dogfood: App Attest credential public key. kuzz-android: Key Attestation leaf SubjectPublicKeyInfo DER.';
COMMENT ON COLUMN push_gateway_installations.assertion_counter IS
    'kuzz-ios-dogfood: App Attest assertion counter. kuzz-android: incremented by one per accepted signed mutation.';
