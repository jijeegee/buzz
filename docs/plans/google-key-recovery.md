# Google onboarding and custodial Nostr key recovery

Status: implementation contract, 2026-10-05. User-approved direction replaces
the token-principal migration in `centralized-identity.md`. This document is
authored before implementation. Implementation evidence and limitations belong
in `google-key-recovery-validation.md`.

## Supersession and scope

This supersedes that plan's G1/G3–G7, identity replacement, agent recreation,
signature/NIP-42/NIP-98/NIP-OA removal, token-only client selection, key-backup
deletion, global profile replacement, and Phase 3/4 deletion roadmap. Its Q7
acceptance of lost agent history is withdrawn. Existing token accounts/data are
preserved for compatibility, never silently converted or merged.

Buzz identity remains the client's secp256k1 keypair. Signed events, pubkey
allowlists, p-tag mentions, NIP-OA owners/siblings, DM/channel membership,
managed-agent keys/profiles/records, and AI conversation mappings keep their
existing meaning. Google authenticates access to a custodial backup only. An
internal account id authorizes storage and is never a messaging identity.
This advances VISION.md's portable, signed identities and mobile-first use.
The explicit tension with VISION_SOVEREIGN.md is optional custody: the operator
and anyone who takes over the Google account can recover the signing key.

No changes to production configuration or databases, credentials, running
preview artifacts, other worktrees, or CLAUDE.md are authorized by this work.

## Reuse and account separation

Reuse verified Google OIDC issuer/audience/nonce/JWKS validation, client PKCE,
state, one-use login codes, session rotation/revocation and OS secure stores.
OIDC start adds `identity_mode=key_backup`. Carry that mode through pending
login and login code. `(provider, subject)` remains the sole external identity;
email is display metadata, never authority. New accounts in this mode are
marked `key_backup`; existing `token` accounts return `account_mode_conflict`.
The reverse transition is also refused. No identities, events or bots are moved.

`AUTH_KEY_BACKUP_ENABLED=true` independently enables OIDC/account sessions and
backup routes. It does not enable token messaging. `AUTH_TOKEN_ENABLED` retains
its existing meaning for old token accounts. Key-backup sessions cannot use
token messaging, token bot creation/adoption, token profiles or token account
deletion. Session status/logout/device management remain usable. Session refresh
never changes the local signing identity, switches transport, or restarts agents.

NIP-11 adds `buzz_key_backup: {version:1, providers:["google"]}` separately from
`buzz_token_auth`. Updated desktop/mobile prefer this new capability for Google
onboarding and retain key-based community configuration. Legacy key auth stays
enabled; existing token-mode records continue to use the existing mode.

## Wire contract (all responses under /auth are no-store)

OIDC complete and `/auth/me` include `identity_mode: "key_backup" | "token"` and
`signing_pubkey: <hex> | null`; `principal_id` is still the account id. Clients
must explicitly require `key_backup` when using this workflow. Refresh preserves
the account and mode; it does not extend the fresh-login window.

All backup routes require a valid server-issued **user** session belonging to a
`key_backup` account. No caller-provided account selector is accepted.

| Route | Contract |
|---|---|
| `GET /auth/key-backup` | `{state:"absent"|"ready", account_id, pubkey:null|hex, version:1}`. Only an authenticated successful `absent` authorizes initial creation. |
| `POST /auth/key-backup/challenge` `{pubkey:hex}` | `{challenge, account_id, url, expires_in:120}`; random 32-byte hex challenge scoped to session, account, pubkey and `initialize`. |
| `POST /auth/key-backup` `{secret_key:hex, proof:<signed Nostr event>}` | Atomic immutable initialization; success `{pubkey, version:1}`. Both signup and explicit existing-key link use this route. |
| `POST /auth/key-backup/restore` `{}` | `{secret_key:hex, pubkey:hex, version:1}`. Bound account only; no absent-to-create fallback. |

Initialization proof: kind 27235, empty content, valid Nostr id and Schnorr
signature, created_at within 60 seconds, exactly one each of these two-element
tags: `u=<challenge response url>`, `method=POST`, `challenge=<challenge>`,
`account=<account_id>`, `action=initialize`. No other tags. Event pubkey equals
the challenge pubkey and the public key derived from the submitted 32-byte
secret. Redis consumes the challenge once, including failed proof attempts.
The URL is the canonical configured HTTPS origin plus `/auth/key-backup`.
Thus possession is bound to account, action, session, target and uploaded key;
an arbitrary public key, email, transplanted proof or replay cannot bind a key.

Challenge, initialize and restore require a session created by completed OIDC
login within 5 minutes, measured by DB time. Older sessions return
`reauth_required` (403): repeat real Google login. Status may use a refreshed
session. All backup routes have a shared 30/min account limit, with exports
additionally 5/min/account; existing login IP limits remain. Redis/store failure
fails closed. Errors include `backup_exists` (409), `key_already_linked` (409),
`account_mode_conflict` (409), `invalid_proof` (400), `backup_missing` (404),
`backup_unavailable` (503). None authorizes generation except successful status
`absent`. Error messages/logs never contain secrets, bearer tokens or bodies.

## Persistence and encryption

Migration adds an account mode with existing rows defaulting to `token`, plus
`account_key_backups`: account PK/FK, unique public key, version, key id, nonce,
ciphertext, creation timestamp. Both migration and desired schema are updated;
register the deployment-wide table in the operator-global registry.

Initialization locks the account in the same transaction that checks mode and
inserts the backup. Account PK and pubkey unique constraints settle concurrent
first-device and cross-account races. Never UPDATE a binding or ciphertext on
retry. A loser gets an explicit conflict, then can restore the winner on a new
device. Existing-key linking never overwrites a different local identity.

Use RustCrypto AES-256-GCM with an OS-random 96-bit nonce per encryption and
authenticated data containing domain, envelope version, account id, public key,
and key id in an unambiguous length-delimited encoding. Encrypt raw 32-byte key;
validate scalar and derived pubkey before encryption and after decryption.
Ciphertext corruption, wrong master key/key id/AAD/version fail closed.
Zeroize temporary secret buffers where practical; no Debug/Display of secrets.

`AUTH_KEY_BACKUP_MASTER_KEY` is exactly 64 hex characters (32 random bytes),
`AUTH_KEY_BACKUP_KEY_ID` is a short nonsecret version label. They are required
when enabled and the master key is kept outside source and DB, e.g. protected
service environment. Missing/malformed config aborts startup. HTTPS public
origin is required; only explicit `AUTH_KEY_BACKUP_ALLOW_INSECURE_LOCAL=true`
permits HTTP on localhost/loopback for isolated tests/development. Fake Google
must never enable custody outside tests. TLS termination is a trusted deployment
boundary; protect the internal listener and disable proxy body/header logging.

Existing password backup and HPKE helpers remain intact. HPKE currently only
seals a client envelope for an unspecified recipient and has no enrollment or
recovery service; adding a recipient infrastructure brings no benefit for this
small custodial MVP. HTTPS plus server AEAD meets the chosen custody contract.

## Client state machine and continuity

1. Explain custody before Google onboarding/link; start real OIDC with mode.
2. Verify completed session mode and call authenticated status. Fail visibly on
   network/auth/corruption/conflict. Never interpret failure as no backup.
3. If ready, restore and verify its derived pubkey matches status. Do not call
   key generation. Store in existing OS secure key storage before activating.
4. If absent, use the current existing key for explicit linking. Fresh signup
   uses the existing client key-generation implementation (desktop may already
   have a bootstrap key). Persist a pending key in OS secure storage before
   upload, so an uncertain response/restart cannot generate a replacement.
5. Obtain challenge, sign with that key, initialize. Activate only after
   confirmed success. On concurrent creation, new-device onboarding may restore
   the winning backup. Established local identity/link mismatch is a visible
   conflict; preserve all local keys, communities and agent data.
6. Persist signed-key community mode. Messaging/agents use the original signed
   routes. Refresh or backup-session expiry only affects backup management.

Mobile keeps a backed community's public-key metadata if the local private key
is missing. Google recovery can fill that missing key only from a ready backup
matching the recorded pubkey; absence or mismatch never authorizes generation.
Its existing community id and settings remain intact.

Desktop lost-key recovery checks any surviving backup binding and verifies
stored NIP-OA attestations against each agent pubkey and the recovered owner
before persisting. Conflicting/invalid attestations fail closed. If agents
remain but neither a backup binding nor a verifiable owner attestation remains,
automatic recovery reports the missing ownership proof and keeps the existing
manual original-key import available. If all keyring metadata is lost and no
local agents remain, the selected Google backup can be restored but the prior
local owner cannot be independently proven; select the original Google account.

Settings expose link/backup and re-login recovery with truthful errors. Existing
token-mode communities/separate principals are detected and explained, not
silently adopted. On dual-mode relays, known old token sessions keep their mode;
an explicit existing-token-account action allows pre-upgrade users to sign back
in. The server accepts only an already-existing token account through that
route. A local Google key-backup binding cannot be downgraded by this action.
The explicit compatibility action also refuses local key-agent adoption on
desktop and refuses replacement of a saved signing-key community on mobile;
existing token-bot records remain usable. It reports a conflict instead of
merging the two identities.
Identity replacement must not rewrite managed-agent pubkeys,
owner attestations, sender allowlists, mention bindings, profile keys, or session
maps. Existing token bots remain existing token bots. Google login cannot
recreate or restart agents. Restoring a human key does not copy local agent
secrets, files or AI sessions to a new machine; ACP restart-resume depends on
the underlying adapter and is not promised.

## Operations, compatibility and limitations

Operator setup: register real Google Web OAuth client and the exact HTTPS
callback; put id/secret, independently generated master key and key id in a
restricted service environment; run reviewed migration on a backed-up instance;
then enable custody. No real credentials are supplied or generated in this work.
The existing configuration names are `AUTH_OIDC_GOOGLE_CLIENT_ID`,
`AUTH_OIDC_GOOGLE_CLIENT_SECRET`, and `AUTH_PUBLIC_URL`; the callback is
`<AUTH_PUBLIC_URL>/auth/oidc/google/callback`. Use this canonical origin in
clients. Set `AUTH_KEY_BACKUP_MASTER_KEY`, `AUTH_KEY_BACKUP_KEY_ID`, and
`AUTH_KEY_BACKUP_ENABLED=true`; leave token messaging disabled unless existing
token accounts need it. Generate the master key with a cryptographically secure
generator (for example, `openssl rand -hex 32`) directly into protected secret
provisioning, never a source file, ticket or shared terminal log. Restrict the
service environment and master-key backup to the service owner. Configure only
the actual reverse proxy in `AUTH_TRUSTED_PROXY_CIDRS` so login IP rate limits
work behind TLS termination.
Keep encrypted DB backups and a separately protected offline master-key backup.
Losing both local key copies and the master key makes DB backups unrecoverable.
Compromised master key plus DB reveals every backup. Google takeover/operator
access can impersonate a user with the recovered key: this is not E2E custody.

This MVP accepts one master key/id; automatic online rotation and multiple-key
keyrings are deferred. Do not replace the configured key/id while rows exist.
Rotation needs a separately reviewed offline decrypt/re-encrypt procedure with
verified backups and atomic cutover; old DB snapshots still require old keys.

Logout/revoking sessions denies future backup exports; it cannot revoke a copied
Nostr key or terminate signed-key agents. Full per-device cryptographic
revocation is deferred. Unlink, key replacement and automated account merge are
not provided. Existing manual key export/password backup/pairing remain.

Web/CLI existing signed and token routes are preserved. New Google custody
signup/link/restore is initially desktop/mobile only. Web/CLI explicitly request
token mode; on a custody-enabled relay this can only log in an already-existing
token account. New subjects get `unsupported_client`, and a key-backed subject
gets `account_mode_conflict`, with an instruction to use desktop/mobile. An
omitted mode keeps the token default on dual-mode relays for older clients;
on custody-only relays the default is key backup. No unsupported client creates
a separate token identity. CLI key env/import remains supported. Closed relay
membership/invites still gate signed-key access; Google backup enrollment does
not bypass community membership or silently grant operator privileges.

## Verification contract

Run failing tests against production seams before implementation, then green:
client creation→backup→second-device restore with same pubkey/signature;
existing-key linking; refresh stability; unauthorized/cross-account denial;
bad/replayed/account-mismatched proofs; concurrent initialization; AEAD wrong
key/AAD/version/corruption; failure without generation; managed-agent/allowlist,
mention, sibling and conversation continuity. Mock Google only in tests. Record
actual Rust/frontend/Flutter checks and every OAuth/native/live-DB blocker;
do not label simulated tests as real Google acceptance.
