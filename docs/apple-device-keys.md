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
- Selection (same as Windows; `DeviceKey::open_or_create` and `open`, so the
  app's launch `device_key::init` and every helper follow it): the key in use
  wins. An existing software key in `~/Library/Application Support/kuzz/device-keys`
  **without** the hardware marker stays in use even when a Secure Enclave key
  exists (a stray key from a test or dev build, or one made by a binary that
  never recorded the marker): the relay has the software key bound. With the
  marker present, the hardware key is used. With no key at all (fresh
  install), a new Secure Enclave key is created, at launch or sign-in.
  Nothing is swapped mid-session. A new hardware key appears only after the
  software key is gone (`DeviceKey::reset`) and the next sign-in binds it.
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

The code sets no access group itself (no team id in Rust): the key lands in
the binary's first keychain access group.

Values for the Kuzz release build:

| What | Value |
|------|-------|
| Bundle id | `app.kuzz.app.demo.preview-88ecaf8be554431e` |
| Team ID | `BQ75Y6QK8F` |
| Keychain access group | `BQ75Y6QK8F.app.kuzz.app.demo.preview-88ecaf8be554431e` |
| Profile | `MAC_APP_DIRECT` (Developer ID), Developer ID Application G2 certificate |

To get `hardware` on macOS:

1. Sign with the Developer ID Application (G2) certificate of team
   `BQ75Y6QK8F`.
2. Download the `MAC_APP_DIRECT` provisioning profile for
   `app.kuzz.app.demo.preview-88ecaf8be554431e` with the keychain sharing
   capability and put it at
   `desktop/src-tauri/profiles/kuzz-developer-id.provisionprofile`
   (gitignored; never commit it).
3. Build with the signed overlay, **only when that profile exists**:

   ```sh
   extra=()
   if [ -f desktop/src-tauri/profiles/kuzz-developer-id.provisionprofile ]; then
     extra=(--config src-tauri/tauri.macos-signed.conf.json)
   fi
   cd desktop && pnpm tauri build "${extra[@]}" ...   # plus the usual --config files
   ```

   `desktop/src-tauri/tauri.macos-signed.conf.json` is a JSON merge patch on
   `tauri.conf.json`:

   ```json
   {
     "bundle": {
       "macOS": {
         "entitlements": "Entitlements.signed.plist",
         "files": {
           "embedded.provisionprofile": "profiles/kuzz-developer-id.provisionprofile"
         }
       }
     }
   }
   ```

   It embeds the profile as `Kuzz.app/Contents/embedded.provisionprofile` and
   switches to `desktop/src-tauri/Entitlements.signed.plist`: the keys of
   `Entitlements.plist` plus

   ```xml
   <key>com.apple.application-identifier</key>
   <string>BQ75Y6QK8F.app.kuzz.app.demo.preview-88ecaf8be554431e</string>
   <key>com.apple.developer.team-identifier</key>
   <string>BQ75Y6QK8F</string>
   <key>keychain-access-groups</key>
   <array>
       <string>BQ75Y6QK8F.app.kuzz.app.demo.preview-88ecaf8be554431e</string>
   </array>
   ```

   Without the profile the build uses the plain `Entitlements.plist` and
   behaves exactly as before (software key). Never add these keys to
   `Entitlements.plist` itself: a restricted entitlement without a matching
   profile stops the app launching. A pipeline that re-signs the bundle
   itself (for example `signed-macos-canary.yml`, which copies
   `Entitlements.plist`) must copy `Entitlements.signed.plist` instead and
   keep the embedded profile when it re-signs. Sidecars in
   `Contents/MacOS` are signed **without** these entitlements: they reach the
   key through the app signer (below).
4. Check the signed app: `codesign -d --entitlements - Kuzz.app` lists the
   three keys, and the app log says `kuzz-desktop: device key ready (hardware)`
   followed by `kuzz-desktop: app signer ready`. After sign-in the relay shows
   `device_key_kind = hardware` for the session. An agent started by the app
   logs `device key not reachable here; signing through the app`.
5. Optional local check of the backend: build the test binary
   (`cargo test -p kuzz-device-key --no-run`), sign it with the same identity,
   entitlements and profile, then run it with
   `--ignored a_secure_enclave_key_signs_proofs_the_server_accepts`
   (creates and deletes a `Kuzz-Device-test-*` key). Unsigned, the non-ignored
   tests pass and use software keys.

### Sidecars: the app signer

`kuzz-acp`, `kuzz`, `git-credential-nostr` and `kuzz-dev-mcp` open the same
device key. On Windows they reach the per-user TPM key. On macOS they cannot
reach the Secure Enclave key (command-line executables cannot carry the
keychain access group without a profile of their own), so the desktop app
signs for the runners it spawns:

- Once the app opened its device key it starts a loopback **app signer**
  (`crates/kuzz-token-broker/src/app_signer.rs`, desktop
  `desktop/src-tauri/src/app_signer.rs`) on `127.0.0.1:<random port>`. It
  runs on every desktop platform; helpers that open the key themselves never
  ask it, so Windows TPM behaviour is unchanged.
- Each spawned `kuzz-acp` gets its own lease in `KUZZ_APP_SIGNER_URL` and
  `KUZZ_APP_SIGNER_SECRET` (random 32-byte bearer secret, constant-time
  compare, scoped to that agent and relay, revoked when the process ends;
  reserved env keys, so a saved agent env cannot set them). The app stops the
  signer, revoking every lease and releasing the key handle, on exit.
- `POST /device-tag`: the device broker request (`{"event": <unsigned>}`) and
  admission checks: kind 22242, 27235 or 24242, authored by the agent, within
  60 s, not already tagged, aimed at the lease's relay. Answers the tag.
- `POST /header-proof`: `{"ts", "purpose", "binding", "token_sha256",
  "body_sha256"}`. The raw token and body never leave the helper, only their
  SHA-256 hex. Purposes `auth` and `refresh` only (`bind` is refused: no
  spawned helper signs in); `token_sha256` required; `ts` within 300 s of the
  app's clock; `binding` is `<METHOD> <http(s) URL>` on the lease's relay.
  Answers `{"proof": "<ts>.<nonce>.<sig>"}`, the `X-Kuzz-Device-Proof` value.
- Bounds: 8 KiB requests, 5 s read limit, 16 connections, 600 requests per
  minute per lease, 256 leases. Unknown paths `404`, bad secret `401`, refused
  content `403`, stopped `503`. The secret is never logged.
- Helper resolution (`DeviceSigner::from_env`): own device key, else the app
  signer from the env, else (agent children) the `kuzz-acp` device broker,
  else none. A signing failure is never fatal: the request goes out without a
  tag or proof, with a warning.
- `kuzz-acp` hands its agent children its own device broker, never the app
  signer secret; when `kuzz-acp` itself signs through the app, the broker
  forwards the tags it admitted to the app signer.

Not covered: a `kuzz` CLI or `git-credential-nostr` started by hand in a
terminal (not by the app) still has no way to the Secure Enclave key on a Mac
and sends no device tag or proof.

### Software to hardware upgrade (not implemented)

A Mac or PC with a software key bound on the relay keeps it (selection
above). Moving it to the Secure Enclave or TPM at a later sign-in is not done,
because all accounts and agents of an install share one device key:

- Other signed-in accounts were bound to the software key. After a switch
  their refresh and `/auth/*` proofs come from the new key and fail with
  `device_proof_invalid` under `enforce`. The desktop treats that as a
  terminal refresh failure that does not end the sign-in (`refresh.rs`), so
  each such account asks to sign in again; its session is lost.
- Agents registered with the software key as `host_device_pubkey` become
  `wrong_host` under `enforce`: `kuzz-acp` exits 77 ("Runs on another
  device") until each is registered again from this machine.

Options, if wanted:

1. Per-session device keys: remember which device key each session and agent
   was bound with and keep both keys until every one has re-bound.
2. Upgrade only when exactly one account is signed in and it hosts no agents;
   otherwise keep software (or ask, then re-sign-in every account and
   re-register agents).
3. A relay re-bind endpoint: prove with the old key, bind the new one, for
   every session and agent at once.

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
