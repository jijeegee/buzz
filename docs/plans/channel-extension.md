# Extend existing channels: discovery and decision gate

2026-10-06. Baseline: `1187dd35f` (`feat(auth): add Google custodial same-key recovery`).
Status: **blocked at the user-requested decision gate, before implementation**.
This is a bounded source inspection, not an exhaustive security audit.

## Contract and preserved scope

Deliver one desktop/mobile chat list containing DMs, real standalone channels,
and joined community channels. Personal pins precede actual latest-message
ordering; ties are deterministic. Preserve read/mute state. Standalone groups
must use existing channel messages, membership, roles, timeline, composer,
attachments, replies, reactions and supported agents. Google login must reach
chat home without community enrollment; retain advanced relay configuration.

Both channel affiliations need owner/admin-controlled all-history or
since-current-join visibility. Existing rooms retain all-history; new rooms
default to since-join. Rejoin establishes a new authoritative cutoff, duplicate
join does not. Enforce current membership and history visibility on server
reads, search, replies, previews, media and agent context. Warn before exposing
prior history. Already downloaded data cannot be recalled.

Read: root AGENTS/TESTING, buzz-db TESTING, VISION, VISION_MOBILE,
VISION_SOVEREIGN, VISION_AGENT, VISION_MODERATION and the custody plan/validation.
Intentional tension: VISION makes the community the mandatory isolated world
for channels and DMs; the approved requirement adds a world outside it. It does
not authorize weakening isolation between existing communities.

Preserve [Google custody](google-key-recovery.md): Google restores the same
signing key; account ids never replace messaging identities. Do not touch keys,
agent execution permissions, live accounts, services, databases or credentials.
No old prototype imports, separate chat stack, new message kind, SSE, synthetic
community membership, or migration rewrites. No push or deployment.

## Current production paths and actual blockers

Paths below are relative to the repository root.

| Concern | Existing implementation and consequence |
| --- | --- |
| Schema | `schema/schema.sql:79,145,203`: channels, channel_members and events have mandatory community foreign keys and community-leading primary keys. Channels may share UUIDs across communities. Channel community is immutable. Making only channel affiliation nullable cannot represent a standalone member or event. |
| Connection and authorization | `crates/buzz-relay/src/tenant.rs::bind_community` binds the host before access; unmapped hosts fail closed. `crates/buzz-auth/src/access.rs::ChannelAccessChecker` requires TenantContext for every membership lookup. Closed-relay admission is an additional community membership gate. There is no outside-community connection scope to select. |
| Create/member/roles/leave | `crates/buzz-relay/src/handlers/side_effects.rs` dispatches existing 9007/9000/9001/9021/9022 commands; creation and joins call scoped channel DB methods. `channel_authz.rs` and `crates/buzz-db/src/store/channel_members.rs` preserve role and last-owner safety. These are the paths to extend, never duplicate. |
| Invitations | `crates/buzz-relay/src/api/invites.rs::claim_invite` calls `claim_relay_invite` and publishes community membership. That URL flow cannot be reused unchanged for a group-only invitation. Direct channel member commands are scoped to the connection's community too. |
| Reads and transport | `handlers/event.rs`, `handlers/req.rs`, `api/bridge.rs`, `crates/buzz-db/src/store/event.rs` and `crates/buzz-search/src/lib.rs` carry the community into writes, reads and search. The thread-window path also has separate relay/DB entry points. Dropping tenant predicates would break the existing isolation contract. |
| History cutoff | `channel_members.rs::add_member` revives removed rows without updating joined_at; auto-membership also has a reactivation path. Duplicate 9021 joins currently return early. `events` has received_at as well as sender-created created_at; event insertion stamps received_at in application code. A reliable cutoff needs transaction ordering with membership, not a client timestamp filter. |
| Media | `crates/buzz-relay/src/api/media.rs` binds the host and enforces relay membership on media reads. A channel/history setting alone does not establish attachment eligibility; the download authorization path must be covered before claiming privacy. |
| Desktop reuse | `desktop/src/features/channels/hooks.ts` overlays actual last-message timestamps; `lib/channelRecency.ts` advances them. Sidebar `AppSidebar.tsx` separates stream/starred/DM sections; `lib/channelSortPreference.ts` already sorts recent messages with stable name/id ties. `lib/channelStarsStorage.ts` and `channelStarsSync.ts` hold personal stars. Existing `ChannelScreen.tsx`/`ChannelPane.tsx` are the conversation renderers. |
| Mobile reuse | `mobile/lib/features/channels/channels_provider.dart` loads joined ids from 39002 and fetches latest messages. `channels_page/body.dart` filters isMember and separates stream/starred/DM lists. `channel_stars/`, `channel_sort/`, existing read/mute providers, `channel_detail_page.dart` and `compose_bar/` are reusable. |
| Client scope | Desktop `src/app/App.tsx` applies a selected community before mounting its keyed app/query subtree. Mobile `lib/app.dart`, shared community/relay providers and star provider depend on active community configuration. Aggregating joined communities and hosting no-community chat requires changes beyond flattening the visible lists. |

**Gate conclusion:** this baseline cannot provide real outside-community
channels through a modest nullable-affiliation or creation-policy patch. It
requires changing the storage/connection authorization boundary across the
existing channel stack and both clients. No such overhaul is approved by this
plan. Preserving a mandatory tenant key by secretly enrolling users in another
community would violate the request, even if its UI were hidden.

## Small alternatives for the parent/user decision

1. Independently implement one list and personal pin controls **within the
   selected community**, on both clients, using the providers and renderers
   above. No backend migration. This is a useful partial milestone already
   permitted by the request, but does not supply standalone groups,
   cross-community aggregation, no-community home or history policy. It has
   not been implemented in this discovery checkpoint.
2. If product scope is explicitly changed, offer existing private channels
   inside a visibly selected community using current member/role/leave flows.
   This is not a standalone-group implementation and must not be presented as
   one. Do not make this substitution without a user decision.

Keeping the complete requirement instead needs an explicit decision to allow
the broader existing-channel scope change. No replacement architecture or
parallel service is proposed here.

## Smallest changes after a scope decision

For the independent list milestone, affect the desktop sidebar/list controls,
mobile channels list/tile and their existing tests. Feed joined rows into one
list; reuse personal stars as pins, pinned rows first and each partition by
last-message time with stable identity ties. Empty rooms sort last. Do not use
metadata updated_at, change read/mute storage or add a second timeline. A
multi-community list must first address community-qualified row identities;
current star payloads index channel ids, which are not globally unique.

For a subsequently approved full change, reuse the listed channel/event/auth
paths, then deliver one working create/invite/send/read flow before extending
all history-sensitive reads. Extend existing channel settings on both clients
with the same policy, owner/admin checks and prior-history warning. Do not
commit disconnected policy helpers as a functioning milestone.

Migration strategy: none in this checkpoint. Any later implementation adds a
new migration after 0059 and updates desired schema; existing migration files
and data remain untouched. Backfill existing rooms to all-history, then set
the new-room default to since-join. Reset the authoritative cutoff only on
inactive-to-active membership under the existing membership lock. Resolve
storage scope and cutoff concurrency before writing a schema migration. Test
desired-schema and migration paths only in new explicit isolated fixtures;
deployment/application belongs to the parent, not this task.

## Acceptance and validation

- Desktop and mobile: actual new messages reorder mixed chats; metadata edits
  do not. Pins remain above unpinned chats and affect only their owner. Ties,
  empty rooms, refresh/reconnect and account/community changes remain stable.
  Unjoined public rooms are absent; read/mute behavior stays intact.
- Standalone channel: an invited account without community membership can
  create/join/use the existing conversation features; invitation grants only
  that channel. Join/leave/kick, role safety and local agent permissions work.
- Policy: old rooms preserve history, new rooms default since-join, owner/admin
  settings and warning work. Leave/kick denies reads; successful rejoin hides
  the old period; duplicate joins preserve cutoff. Concurrent join/send and
  malicious sender timestamps cannot bypass it. Exercise raw signed queries,
  search, thread roots/replies, previews, downloads and agent context.
- Isolation: same channel UUID in two communities, foreign event/media ids,
  open-channel discovery and cross-account pins cannot cross boundaries.
- Google recovery preserves user/agent keys and ownership. Ordinary login
  needs no IP/WSS entry and reaches chat home with zero communities.
- Focused production-path tests plus actual changed app/relay flows precede
  functioning local `git commit -s` milestones. Explicitly stage only changed
  task files. Human acceptance remains pending until the user tests it.

Actual work and checks: [validation record](channel-extension-validation.md).
