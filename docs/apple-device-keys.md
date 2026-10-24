# Apple device keys (macOS and iOS Secure Enclave)

Device-bound keys keep each install's P-256 device key in hardware where the
platform has it: the TPM on Windows (`crates/kuzz-device-key/src/tpm.rs`), the
Android Keystore (`mobile/android/.../DeviceKeyPlugin.kt`), and the Secure
Enclave on Apple platforms (this note). The relay records the result as
`device_key_kind = hardware | software`.

## macOS desktop (`crates/kuzz-device-key/src/secure_enclave.rs`)

- Key: ECDSA P-256 created in the Secure Enclave with `SecKeyCreateRandomKey`,
  `kSecAttrTokenIDSecureEnclave`, permanent in the **data protection keychain**,
  label `Kuzz-Device-kuzz-device-v1`, access `AfterFirstUnlockThisDeviceOnly` +
  `privateKeyUsage`. Signatures: `ecdsaSignatureDigestX962SHA256` over the
  SHA-256 digest, DER converted to raw `r || s` (same bytes form as TPM and
  software keys).
- Order (same as Windows): existing Secure Enclave key, else an existing
  software key in `~/Library/Application Support/kuzz/device-keys`, else a new
  Secure Enclave key. A Mac that already has a software key keeps it; nothing
  is swapped mid-session. A new hardware key appears only after the software
  key is gone (`DeviceKey::reset`) and the next sign-in binds it.
- Software fallback only when the Secure Enclave is unavailable to the binary:
  `errSecMissingEntitlement` (-34018), `errSecUnimplemented` (-4),
  `errSecNotAvailable` (-25291), or any creation failure on an Intel Mac. Any
  other failure (for example the keychain is locked, -25308) is an error that
  the app retries; it never makes a software key.
- Once a Secure Enclave key was used, the store gets the hardware marker
  (`kuzz-device-v1#tpm.key`, shared name with Windows). With the marker
  present, a binary that cannot reach the Secure Enclave gets an error, not a
  new software key.

### Signing requirements (Mac build agent)

The data protection keychain only works for a binary signed with a keychain
access group. Without one (unsigned or ad-hoc builds, `tauri dev`, `cargo test`)
every call returns -34018 and the app quietly uses a software key, exactly as
before this change.

To get `hardware` on macOS:

1. Sign with a Developer ID (or Apple Development) certificate of the Kuzz team.
2. Create a provisioning profile for the app id `app.kuzz.app` (Developer ID
   profile for distribution) and embed it as
   `Kuzz.app/Contents/embedded.provisionprofile` (Tauri:
   `bundle.macOS.files: { "embedded.provisionprofile": "<path>" }`).
3. Add to `desktop/src-tauri/Entitlements.plist` (only together with the
   profile; an unauthorised restricted entitlement stops the app launching):

   ```xml
   <key>com.apple.application-identifier</key>
   <string>TEAMID.app.kuzz.app</string>
   <key>com.apple.developer.team-identifier</key>
   <string>TEAMID</string>
   <key>keychain-access-groups</key>
   <array>
       <string>TEAMID.app.kuzz.app</string>
   </array>
   ```

4. Check the signed app: `codesign -d --entitlements - Kuzz.app` lists the
   three keys, and the app log says `kuzz-desktop: device key ready (hardware)`.
   After sign-in the relay shows `device_key_kind = hardware` for the session.
5. Optional local check of the backend: build the test binary
   (`cargo test -p kuzz-device-key --no-run`), sign it with the same identity,
   entitlements and profile, then run it with
   `--ignored a_secure_enclave_key_signs_proofs_the_server_accepts`
   (creates and deletes a `Kuzz-Device-test-*` key). Unsigned, the non-ignored
   tests pass and use software keys.

### Known gap: sidecars and the CLI

`kuzz-acp`, `kuzz`, `git-credential-nostr` and `kuzz-dev-mcp` open the same
device key. On Windows they reach the per-user TPM key. On macOS they can only
reach the Secure Enclave key if they are signed with the same keychain access
group, which command-line executables cannot carry without a provisioning
profile of their own. After the app made a Secure Enclave key they therefore
get an error and send no device tag or proof (best effort, as with any device
key failure). This must be solved (sign the helpers as bundled helper apps with
the group, or let the desktop app sign for them) before the relay enforces
device proofs for agent traffic from Macs.

## iOS (`mobile/ios/Runner/DeviceKeyPlugin.swift`)

- Same `kuzz/device_key` channel and replies as Android, so the Dart flow is
  shared (`mobile/lib/shared/device_key/device_key.dart` now enables iOS):
  the key is created on the first `info()` (sign-in start), its public key and
  kind go to sign-in start/complete, and WebSocket auth, token refresh and
  media passes carry proofs.
- Key: Secure Enclave P-256, application tag `kuzz.device-key.v1`,
  `AfterFirstUnlockThisDeviceOnly` + `privateKeyUsage`, signed with
  `ecdsaSignatureMessageX962SHA256` (DER, converted to raw in Dart).
- The Simulator has no Secure Enclave: there the key is an ordinary keychain
  key and is reported as `hardware: false`. On a device a Secure Enclave
  failure is an error that Dart retries, never a software key. An existing key
  of either kind is always reused.
- Nothing extra to sign: the Runner already has `keychain-access-groups`
  (`$(AppIdentifierPrefix)$(KUZZ_KEYCHAIN_ACCESS_GROUP)`); the key lands in that
  first group.
- Unlike Android, iOS keychain items survive deleting the app, so a reinstall
  on the same iPhone finds the same device key.
- Check on a device: sign in, and the relay shows `device_key_kind = hardware`;
  the Simulator shows `software`.
