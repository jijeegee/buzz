//! Domain-owned persistence implementations.

/// Explicit deployment-global admin report reads.
pub mod admin_moderation;
/// Community-scoped authentication allowlist persistence.
pub mod allowlist;
/// API token storage and lookup.
pub mod api_token;
/// Relay-scoped archived identity persistence (NIP-IA).
pub mod archived_identities;
/// Silent user blocks.
pub mod blocks;
/// Channel lifecycle and metadata persistence.
pub mod channel;
/// Channel membership and roster persistence.
pub mod channel_members;
/// One-off backfill of server-side chat state from message history.
pub mod chat_backfill;
/// First-run upload of device-local read positions and follows.
pub mod chat_import;
/// Chats list for one reader, read from server-side chat state.
pub mod chat_list;
/// Server-side chat state written with each message (read positions, thread
/// follows, thread close state).
pub mod chat_state;
/// Reader-driven chat state: read positions, unread marks, follows.
pub mod chat_user_state;
/// Community lifecycle and host-map persistence.
pub mod community;
/// Durable whole-community deletion lifecycle and PostgreSQL adapter.
pub mod deletion;
/// Direct message channel persistence.
pub mod dm;
/// Event storage and retrieval.
pub mod event;
/// Home feed queries.
pub mod feed;
/// Git repository name registry (NIP-34 kind:30617).
pub mod git_repo;
/// Centralized identity: principals, devices, sessions, bots, access tokens.
pub mod identity;
/// One-statement inbox summary of member rooms and active threads.
pub mod inbox_summary;
/// Attachment permissions: uploaders and event references.
pub mod media_access;
/// Community moderation: reports, bans/timeouts, audit actions.
pub mod moderation;
/// Deployment-global operator-listener mention registrations and delivery queues.
pub mod operator_listener;
/// Monthly table partition management.
pub mod partition;
/// Exact person lookup and shared-channel people search.
pub mod people;
/// Kuzz product-feedback sidecar persistence.
pub mod product_feedback;
/// Community-scoped push lease and durable wake-outbox persistence.
pub mod push;
/// Reaction persistence.
pub mod reaction;
/// Complete own-author read-state snapshots.
pub mod read_state;
/// HTTP report-resolution enforcement state machine persistence.
pub mod relay_admin_actions;
/// Use-limited relay invite persistence (v2 opaque tokens).
pub mod relay_invite;
/// Relay-level membership persistence (NIP-43).
pub mod relay_members;
/// Deployment-global relay operator/moderator roster persistence.
pub mod relay_operators;
/// Event-reminder delivery query, claim, and release persistence.
pub mod reminder;
/// Deployment regions (migration 0078).
pub mod region;
/// Replaceable-event persistence and coordinate locking.
pub mod replaceable;
/// Durable completed snapshots from the isolated media-storage worker.
pub mod side_effect_outbox;

pub mod storage_accounting;
/// Thread metadata persistence.
pub mod thread;
/// Strict newest-first thread windows and auxiliary scans.
pub mod thread_window;
/// Per-community usage rollup queries for Prometheus gauges.
pub mod usage;
/// User profile persistence.
pub mod user;
/// Workflow, run, and approval persistence.
pub mod workflow;

/// NIP-AR artifact lifecycle and durable delivery.
pub mod artifact;

mod artifact_query;

#[cfg(test)]
mod artifact_postgres_tests;

#[cfg(test)]
mod blocks_postgres_tests;

#[cfg(test)]
mod media_access_postgres_tests;

#[cfg(test)]
mod channel_history_postgres_tests;

#[cfg(test)]
mod channel_sequence_page_postgres_tests;

#[cfg(test)]
mod chat_backfill_postgres_tests;

#[cfg(test)]
mod chat_import_postgres_tests;

#[cfg(test)]
mod chat_list_postgres_tests;

#[cfg(test)]
mod chat_state_postgres_tests;

#[cfg(test)]
mod chat_stored_state_postgres_tests;

#[cfg(test)]
mod inbox_summary_postgres_tests;

#[cfg(test)]
mod side_effect_outbox_postgres_tests;
