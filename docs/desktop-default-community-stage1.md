# Desktop default community — stage 1

Scope: after identity, SetupStep and DefaultConfigStep (including their existing
skip-later branches), automatically connect the first community at the configured
Google custody service origin. Keep profile/team setup and the existing app.
Configured communities, recovered signing keys and advanced manual recovery stay
intact. No channel, PIN, groups, history, mobile or server deployment work.

## Plan and focused risk review

1. Quarantine interrupted standalone edits outside the repository with verified
   SHA-256 copies, then restore only those files to HEAD. Preserve CLAUDE.md and
   committed history groundwork; do not deploy its schema.
2. Replace the ordinary no-community chooser with automatic ingress into the
   existing persisted community-onboarding transaction. Read the service origin
   through native auth status; do not hardcode a deployment hostname.
3. Wait for the exact backend community configuration, then complete the existing
   signed WebSocket authentication before profile setup/app entry. Preserve a
   failed transaction with explicit retry and advanced manual recovery.
4. Run focused production-flow tests and TypeScript checking, reuse the verified
   native binary if unchanged, and launch the isolated DEV app. Commit only this
   stage with DCO signoff; no push.

Review: the real risks are mistaking a URL for membership, advancing after failed
auth/apply, stale async completion after retry/cancel, and changing a configured
user's community or signer. Resolve these through the normal host boundary and
signed auth, the existing applied-key gate, cancellation fences, durable errors,
and limiting automatic ingress to an empty community list. SetupStep,
DefaultConfigStep and Google recovery are not edited. This follows VISION.md and
VISION_SOVEREIGN.md: one normal community per host, portable identity.

## Server prerequisite established from public metadata and source

On 2026-10-06, `https://jihobuzz.duckdns.org:8443` advertised community
`0eaa0496-74c9-4d86-bccc-c938743932ab`, a stable relay `self`, signed authentication,
Google key backup, and no NIP-43. `nip11_facts` advertises NIP-43 exactly when a
stable-key relay requires membership. `check_membership` admits signed users on
this existing open relay; custody endpoints do not themselves enroll users.
Thus this deployment needs no new enrollment handler or server change. The
client must still wait for actual signed authentication; it must never invent a
membership row. A closed relay must admit the same key using its existing invite
or membership policy; denial remains visible and retryable.

Automated/mock validation, native runtime evidence, and the human's real Google
login acceptance are recorded separately. Existing login/data must not be reset
to manufacture a first-run test.

## Verification

- Focused review: minimalism, elegance and correctness each 9/10; the concrete
  failure cases above are covered, with no remaining blocking finding.
- Seven production-flow Playwright tests passed: Google setup-later; model save
  and skip; exact backend-apply wait; signed AUTH denial retained across reload
  and retried with the same key/community; apply-failure retry; advanced recovery
  with late apply; and preservation of a configured user's community.
- Thirty related onboarding/runtime unit tests, TypeScript compilation,
  `build:e2e`, changed-file Biome and diff whitespace checks passed.
- The existing provider-method/harness-filter E2E also passed (eight focused
  browser tests total); SetupStep and DefaultConfigStep are byte-unchanged.
- Windows validation used installed Python (`python3` was unavailable) and cached
  Playwright Chromium 1208 through an external local config. Hermit activation
  failed at `/pkg/hermit@stable/hermit`; this checkout had no installed Git hooks.
  No hook was disabled. The broad file-size ratchet reports pre-existing +1-line
  growth in `managed_agents/discovery/tests.rs` and `managed_agents/readiness.rs`
  against its merge base; neither file is changed by this stage.
- Real native DEV evidence: the existing Google session remained active, the
  current signer matched its recovered signing key, one default community owned
  that key, WebSocket state was `connected`, and the persisted transaction reached
  `default-community / profile` with no error. The native window showed the
  existing Build your profile screen. No login, profile or app-data reset was
  performed. Subsequently, on 2026-10-06, the user personally reported successful
  login into the app, channel creation and message sending. This is user-reported
  acceptance of that flow; no additional login/recovery test by the agent is
  implied.
- Native sources are unchanged; the dedicated binary matched its recorded
  SHA-256 and was reused. The launcher reported `externalJob: false` and its DEV
  native/Vite children were live after the launching command exited. Production
  app and servers were untouched. No history feature or schema was deployed.
