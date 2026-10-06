# Extend the existing channel stack

2026-10-06. Baseline `1187dd35f`; planning HEAD `67ba765a7`.
The user approved the necessary storage, connection and authorization changes.
The former decision gate is superseded. Deployment remains out of scope.

## Contract

Google same-key recovery opens chat home with zero communities. Desktop and
mobile show one list of DMs, groups and joined community channels. Personal
stars become pins; each partition sorts by actual last message and stable scoped
identity. Empty rooms last; metadata edits do not count. Preserve read/mute,
drafts and pending sends. Default onboarding has no IP/WSS entry; retain custom
relay entry under advanced settings. A group is an existing channel with absent
community affiliation, using the existing commands, membership/roles, Nostr
kinds/WS, timeline/composer, attachments/replies/reactions and supported agents.
No separate chat stack, synthetic community or implicit community membership.

Read: AGENTS, VISION, VISION_MOBILE, VISION_SOVEREIGN, VISION_AGENT,
VISION_MODERATION, root/buzz-db TESTING and Google recovery plan/validation.
Intentional extension: optional community affiliation. Existing community
isolation, signing identities, recovery and local agent execution rules remain.

## Chosen storage and routing

Use additive migrations from 0060 and update desired schema/reconciliation. Add
`channel_scopes(id UUID PRIMARY KEY, community_id UUID UNIQUE NULL REFERENCES
communities(id), host TEXT NOT NULL UNIQUE)`. Copy existing community ids/hosts
unchanged; affiliated rows require id = community_id. Community creation/host
updates synchronize the registry transactionally. Host uniqueness must cover
both registries, including future inserts. An explicitly registered standalone
host has a random scope id and NULL affiliation; no communities or relay_members
row is created for it or its users.

Repoint only shared channel-store FKs to channel_scopes: channels,
channel_members, events, users, event_mentions, thread_metadata, reactions,
audit_log, parameterized_event_watermarks and the existing shared feature tables
needed by enabled operations. Retain composite keys, values, indexes and scoped
predicates. For compatibility their legacy SQL `community_id` column remains
the storage partition key; actual affiliation is channel_scopes.community_id.
API responses must not label a standalone partition as a community. Community
membership/invites, bans, deletion and operator authority retain real community
FKs. This avoids a bulk rename of existing channel queries, not an extra product.

Extend the resolved connection context with explicit optional affiliation.
Shared channel routes resolve a registered host to exactly one partition;
unknown hosts fail closed. Existing community-only routes still demand a real
community context. Reuse root WS and /events, /query, /count handlers on the
standalone host, and shared media/window/profile/agent routes. Deny community-only
routes and kinds there. Standalone admission proves NIP-42/NIP-98 identity and
retains configured identity checks/rate limits without requiring relay_members.
Never disable the existing community gate globally or resolve by bare channel id.
Keep partition-qualified Redis/caches. Community deletion/write fences remain
mandatory for affiliated scopes; standalone writes validate their registered
scope and never manufacture a community deletion lease.

Configuration: `BUZZ_STANDALONE_RELAY_URL` explicitly registers the standalone
host at startup (never on arbitrary inbound hosts). Reject conflicts with an
affiliated host. If it equals the canonical relay URL, skip community/owner/
allowlist bootstrap there; otherwise retain canonical community bootstrap and
register the additional host. Advertise the standalone URL as a NIP-11 capability
at the configured login service. Clients use it with their recovered key; custody
sessions remain bound to their original auth origin. No production config changes.

## Membership and invitation

Use 9007 creation/creator-owner bootstrap, 9000 add/roles, 9001 remove, 9021 join,
9022 leave and current last-owner guards. Community channels still require
community eligibility, including target pubkeys. Groups grant only membership
of that channel. Implement the currently deferred 9009 invitation in the same
command dispatcher. `channel_invites` has scope/channel/target-pubkey key,
inviter, expiry and acceptance state. Targeted invitations need no bearer code.
Reuse p-gated membership notification conventions for invitation delivery.
Accept via 9021: check target, expiry, current inviter authority and eligibility,
then consume invitation and add/reactivate member in ONE transaction under the
existing membership lock. Duplicate accept is idempotent; consumed invitations
cannot authorize rejoin. Pending invites permit only minimal invitation metadata.

Standalone private rooms require invitation; open rooms can be discovered/joined
by authenticated keys on that host. Open community discovery remains community
scoped and subject to community admission. Neither exposes message content to
nonmembers. Home includes joined rooms only. No cross-partition open predicate.

## History enforcement, before groups are enabled

Add `channels.history_policy` ('all'/'since_join'): existing rooms backfilled
'all', new-room default 'since_join'. Owner/admin changes use 9002 and 39000;
both clients warn before enabling all-history. Add channel-local server ordering:
`channels.accepted_sequence`, `events.channel_sequence`, and
`channel_members.join_sequence`, all BIGINT default 0 before legacy backfill.
Event acceptance and joining
acquire the SAME existing membership advisory lock, then advance/read the
channel counter inside their transaction. Never allocate order before the lock.
Visible content requires active membership and (all-history OR event sequence
> current join sequence). New membership/reactivation stamp DB joined_at after
the lock; duplicate joins/role changes preserve cutoff. Cover direct add, invite
acceptance, creator bootstrap and automatic membership. Rejoin never revives an
old membership period. Client created_at cannot establish eligibility.

Backfill events in deterministic (received_at, id, created_at) order per channel and seed
the high watermark. Old reactivation discarded removed_at while keeping stale
joined_at, so legacy rows cannot prove their current membership period. Mark
their cutoff as migration-established and conservatively use that watermark.
Existing rooms remain all-history. On switching a legacy room to since-join,
explicitly warn that those members see only post-migration content because old
membership periods cannot be reconstructed. This is a compatibility limitation,
not reconstructed historical truth. Future joins/rejoins have exact cutoffs;
duplicate joins preserve the legacy marker.

Persist 9009 and invited 9021 through a narrow atomic ingest
branch, following persist_workflow_deletion. Do not use catch-and-log post-store
side effects for authoritative mutation. Command, invite consumption and membership
commit together before ACK. Pending invites remain queryable from durable rows;
notifications/publication repair are durable, not the sole discovery mechanism.

Push membership/history predicates before LIMIT/count/aggregation in REQ,
HTTP reads, search, windows/threads, inbox previews and unread derivation. Check
authoritative membership/policy on the writer, including fan-out and stale
cache/replica paths. Discovery snapshots/minimal pending-invite metadata are
separate from message-history visibility. Edits/reactions/replies/summaries and
agent context must also authorize referenced messages/root; reject replies to
invisible roots. Counts and summaries must not reveal hidden history.
Media needs an accessible referencing event in the same scope, or the uploader's
own unattached staging upload, including thumbnail/HEAD/range/download paths.
A hash is not permission. Legacy unmapped media fails conservatively, never
public fallback. Agent context uses the same authorized reads; tool execution
permissions stay unchanged. Previously downloaded bytes cannot be recalled.

## Client sessions

Extend existing saved connection config with explicit standalone/community
kind; do not persist a fake community. Google custody selects a standalone
connection with the recovered key. Keep per-session transports and existing
conversation renderers. Row/route identity is (normalized relay URL, scope kind,
channel id). Aggregate joined summaries from all configured connections with
bounded independent retries; an offline community cannot block chat home.
Selecting a row activates its real connection before opening existing desktop
ChannelScreen/ChannelPane or mobile channel_detail_page. Fence async results;
retain resetCommunityState on active-session changes. Never send on the previous
connection. Adapt desktop/native commands and mobile providers for zero-community
home. Preserve per-connection star/read/mute data; remote stars remain scoped by
their own transport. Scope profiles and previews too. An active-community-only
flattened list is not fulfillment of the mixed-home requirement.

## Checkpoints and validation

1. One bounded read-only independent plan review; resolve concrete blockers here,
   then implement immediately without another approval gate.
2. First functioning checkpoint prioritizes isolated create -> targeted invite ->
   accept by a zero-community identity -> send/read through existing channel
   handlers AND conversation UI. Enforce membership/history from the first
   enabled path. Test wrong host, colliding ids, uninvolved/removed keys, duplicate
   accept and concurrent join/send. Helpers alone are not this checkpoint.
3. Complete protected references/media/search/context and both settings UIs;
   check old/new rooms, leave/kick/rejoin, duplicate join, malicious timestamps
   and every actual read surface.
4. Complete Google-to-home, all-connection mixed list/pins and mobile parity;
   test reconnect/offline connection, colliding ids, account/session switches,
   unread/mute/drafts and unchanged keys/agent ownership.

Use proportional production-path tests and isolated relay/frontend fixtures.
Update migration expectations and scoped-table lint inventory. Exercise desired
schema AND additive migration against new explicit DB/Redis fixtures only.
Record review resolutions, real checks, commits and outstanding acceptance in
[channel-extension-validation.md](channel-extension-validation.md). Commit
functioning increments locally with `git commit -s`, explicit owned file staging
and staged diff inspection. Preserve CLAUDE.md exact bytes/exclude it. Honor
hooks; report genuine resource/tool blockers promptly. No push, deployment,
live migrations/accounts/keys, service/native restart/rebuild, cache deletion or
unrelated process termination. Human/deployed acceptance remain pending.
