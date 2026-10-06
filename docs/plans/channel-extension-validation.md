# Channel extension validation

2026-10-06 discovery checkpoint. Baseline `1187dd35f` on
`feat/centralized-identity`. **No product implementation or functioning feature
milestone is claimed.** The requested decision gate stopped large edits.

## Actual changes and checks

- Added `channel-extension.md` with current-code mapping, blockers, minimal
  partial alternatives, migration constraints and complete acceptance scope.
- Added this validation record. No application, relay, schema, migration,
  authentication, agent or test source changed.
- Inspected the host binding, schema keys, channel create/member/join/leave
  handlers, role safeguards, community invitation claim, read/search/media
  entry points, client conversation/list/personal-star providers and custody
  contract. This demonstrates reuse candidates, not implemented reuse.
- Confirmed initial HEAD and working tree: only pre-existing `CLAUDE.md`
  modified. Its exact empty bytes are protected by SHA-256
  `E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855`.
- Passed: 28 mapped file/link existence checks, documentation content review,
  `git diff --check` and `git diff --cached --check`. Staging contains only the
  two new documents. The final CLAUDE hash matches the initial hash.
  No builds, app sessions, database fixtures or
  behavioral tests are needed or claimed for this documentation-only change.
- Hermit activation was attempted before Git. The checked-in launcher does
  not select a state directory for Git Bash/MSYS and fails resolving
  `/pkg/hermit@stable/hermit`. Native Git remains available. Only sample hooks
  are installed and core.hooksPath is unset. No hook was disabled or rewritten;
  no dependency/cache cleanup was performed.

## Remaining acceptance

All three product requirements remain unimplemented by this checkpoint.
Source inspection found mandatory community keys across channels/members/events
and host-bound authorization, rather than a nullable channel affiliation seam.
The independent selected-community list/PIN milestone remains available;
substituting community-bound private channels for standalone groups needs an
explicit product decision. Full outside-community channels require permission
to exceed the requested modest-extension scope, not a hidden workaround.

There is no new user flow to test yet. After a usable milestone, the highest
value human checks are: message-based mixed-list ordering and personal pins on
two accounts; group-only invitation for a zero-community account; and leave /
rejoin with history settings, search and attachments on desktop and mobile.
No human acceptance is marked complete.

No push, deployment, service restart, migration application, production access,
credential read, keyring operation, logout, reinstall or native forced restart.
Neither protected server nor any existing database/volume/proxy was touched.
Quarantined work and previous architecture outside the restored repo were not
read or imported. `CLAUDE.md` is excluded from staging and commits.
