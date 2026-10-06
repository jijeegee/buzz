# Channel extension validation

2026-10-06. Baseline `1187dd35f`, planning HEAD `67ba765a7` on
`feat/centralized-identity`. The user approved storage/connection/auth changes;
the former decision gate is superseded. No deployed or human acceptance.

## Plan review

One independent read-only Codex review completed. Minimalism 9/10, elegance
9/10, correctness initially 7/10 with three concrete blockers. Parent reviewed
all findings and incorporated these fixes in channel-extension.md:

- Backfill deterministic server receipt order. Old reactivation discarded the
  current-period evidence, so mark legacy cutoffs conservatively at migration
  and require a settings warning; never claim to reconstruct lost join history.
- Persist invitation/acceptance atomically with its command before ACK; durable
  invite discovery/retry must survive notification failure.
- Register standalone hosts explicitly and skip community/owner bootstrap for
  a standalone canonical host. Retain existing affiliated bootstrap and fences.

The review also confirmed scoped IDs/caches, joined-only content, media GET/HEAD,
filtered summaries and activation-before-send as required implementation seams.
No further review or authorization gate was introduced.

## Implemented database ordering increment

Migration 0060 and desired schema now store history policy and server acceptance
order. Existing rooms retain all-history; new rooms store since-join as their
future policy default. Event insertion and membership creation/reactivation
share the existing membership advisory lock. Duplicate joins and role changes
preserve cutoff and joined_at; real rejoin, including automatic membership,
advances both. This uses the existing channel/member/event tables and writers.

This increment is persistence groundwork. It DOES NOT yet enforce history in
user reads or provide standalone create/invite/accept/UI. No complete user flow
or privacy feature is claimed from these tests alone.

## Real checks

All database checks use newly created disposable local fixtures only:
`buzz-channel-extension-pg-20261006` on localhost:55467 (tmpfs, 384MB limit),
`buzz-channel-extension-redis-20261006` on localhost:56467 (96MB limit).
Desired schema: `channel_extension_fixture`. Migration path:
`channel_extension_migration_fixture`, initially empty, applied 0001-0059,
seeded through real writers, then applied 0060. No existing database reused.

- Cached native cargo 1.95.0, offline, two jobs; core test compilation passed.
- Three new desired-schema production-writer tests passed: sender timestamps
  cannot change acceptance order; duplicate/rejoin/automatic membership cutoff;
  an uncommitted event holds the membership lock until commit.
- One additive migration test passed: legacy order and all-history preserved,
  conservative legacy marker retained on duplicate join, new-room default and
  continuing event sequence correct.
- Existing channel membership PostgreSQL suite: 20/20 passed, including owner
  safety, role changes, locks, roster publication and same-key tenant scoping.
- Embedded migration inventory test passed; cargo fmt --all -- --check passed.
- PostgreSQL discovery checker passes for buzz-db (57 Rust files). The required
  full scripts/test-postgres-test-discovery.sh ran: its fixture checks passed,
  then it failed on pre-existing auth/live_tests modules outside postgres_tests
  in buzz-relay. No new test was reported by that failure. Not silently skipped.
- Hermit activation was attempted before Git. Git Bash fails resolving
  /pkg/hermit@stable/hermit; WSL bootstrap could not connect to GitHub. Native
  installed tools/caches were reused. core.hooksPath is unset and only sample
  hooks are present; no hook was disabled or rewritten.

## Outstanding product acceptance

Still required: standalone storage/host/auth boundaries; atomic targeted invite
and acceptance by a zero-community key; end-to-end existing conversation UI;
all actual read/ref/search/media/preview/agent-context guards; owner/admin policy
settings and warnings on desktop/mobile; Google-to-home; all-connection mixed
list/pins and preserved read/mute/drafts. An active-community inbox is not a
substitute. The next meaningful user-facing checkpoint remains
create -> invite -> accept -> send/read with no community membership.

Legacy limit: pre-migration joined_at may describe an older membership period.
Old rooms keep all-history. A later since-join change must disclose that legacy
members see post-migration content only; future joins/rejoins have exact order.

No push, deployment, existing-service/native restart/rebuild, live migrations,
real account/key/OAuth/proxy access, cache deletion or unrelated process kill.
CLAUDE.md remains the original empty modified file, excluded from staging:
SHA-256 E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855.
Human testing and deployed acceptance remain pending.
