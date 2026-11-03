//! iOS user-stream shadow report: `POST /api/user-stream/shadow-report`
//! (plan V4.2 U1d, docs/message-sync.md#counters).
//!
//! iOS has no log anyone can read, so the app posts its user-stream report
//! line here at most once every 5 min. The relay authenticates it like the
//! HTTP bridge (`/query`): a centralized-identity `Bearer` token, or NIP-98
//! with NIP-FI admission and replay protection, then the relay-membership
//! gate. It accepts exactly the body the app sends (fixed fields, every
//! number a u32, unknown fields refused, at most 4096 bytes), allows one
//! report per account per 5 min and only logs it: nothing is stored.

// Handlers return axum `Response` errors directly (axum's design).
#![allow(clippy::result_large_err)]

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::api::{api_error, groups};
use crate::state::AppState;

/// The route.
pub const PATH: &str = "/api/user-stream/shadow-report";
/// Largest accepted body (the app does not send a larger one).
pub const MAX_BODY_BYTES: usize = 4096;
/// One report per account per this window.
pub const WINDOW_SECS: u64 = 300;
/// The only platform that reports here (other apps have a readable log).
pub const PLATFORM: &str = "ios";
/// Longest accepted `app_version`.
const MAX_VERSION_LEN: usize = 32;

/// The report body. Field names are the app's (`userStreamReportBody`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowReport {
    /// `ios`.
    pub platform: String,
    /// The app's visible version, `MAJOR.MINOR.PATCH`.
    pub app_version: String,
    /// Report lines logged so far by the app (a gap shows lost counts).
    pub seq: u32,
    /// Cumulative counters.
    pub counters: Counters,
    /// Stream CLOSEDs by `closedReasons` key (`overflow` split).
    pub closed: Closed,
}

/// `counters` (docs/message-sync.md#counters), in report order.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[allow(missing_docs)]
pub struct Counters {
    pub events_compared: u32,
    pub chunk_only: u32,
    pub stream_only_unexpected: u32,
    pub stream_only_expected: u32,
    pub predicted_skips: u32,
    pub verified_skips: u32,
    pub false_skip: u32,
    pub live_rooms_checked: u32,
    pub live_missed: u32,
    pub skip_audited: u32,
    pub skip_audit_new: u32,
    pub skip_audit_failed: u32,
    pub seq_write_skipped_order: u32,
    pub reconnects: u32,
    pub snapshot_ms: u32,
}

/// `closed`: the client's `closedReasons` keys.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(missing_docs)]
pub struct Closed {
    pub off: u32,
    pub not_enabled: u32,
    pub invalid: u32,
    pub too_many_rooms: u32,
    pub restricted: u32,
    pub auth_required: u32,
    pub shadow_full: u32,
    pub full: u32,
    pub lagged: u32,
    pub closed: u32,
    pub other: u32,
}

/// Parse and check a body; `Err` is the 400 message.
pub fn parse(body: &[u8]) -> Result<ShadowReport, String> {
    let report: ShadowReport =
        serde_json::from_slice(body).map_err(|e| format!("invalid report: {e}"))?;
    if report.platform != PLATFORM {
        return Err("invalid report: platform must be ios".into());
    }
    if !is_version(&report.app_version) {
        return Err("invalid report: app_version must be MAJOR.MINOR.PATCH".into());
    }
    Ok(report)
}

/// `MAJOR.MINOR.PATCH`, ASCII digits only (bounded, so it is safe to log).
fn is_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    version.len() <= MAX_VERSION_LEN
        && parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

fn error(status: StatusCode, message: &str) -> Response {
    api_error(status, message).into_response()
}

/// `POST /api/user-stream/shadow-report`: 204 once logged; 400 for a body the
/// app would not send; 429 within 5 min of the account's last report.
pub async fn shadow_report(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let caller = match groups::authenticate(&state, &headers, "POST", PATH, None, Some(&body)).await
    {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    let report = match parse(&body) {
        Ok(report) => report,
        Err(message) => return error(StatusCode::BAD_REQUEST, &message),
    };
    let pubkey = hex::encode(&caller.pubkey);
    let key = format!("user-stream-report:{}:{pubkey}", caller.tenant.community());
    match state
        .admission_rate_limiter
        .check_key(&key, WINDOW_SECS, 1)
        .await
    {
        Ok(result) if result.allowed => {}
        Ok(result) => {
            return error(
                StatusCode::TOO_MANY_REQUESTS,
                &format!("rate limited; retry in {}s", result.reset_in_secs),
            )
        }
        Err(e) => {
            tracing::warn!(error = %e, "user-stream report rate limiter unavailable; denying");
            return error(StatusCode::SERVICE_UNAVAILABLE, "rate limiter unavailable");
        }
    }
    log_report(&caller.tenant.community().to_string(), &pubkey, &report);
    StatusCode::NO_CONTENT.into_response()
}

/// One log line per report; every label is a struct field name.
fn log_report(community: &str, pubkey: &str, r: &ShadowReport) {
    let (c, k) = (&r.counters, &r.closed);
    tracing::info!(
        target: "kuzz_relay::user_stream_report",
        community,
        pubkey,
        platform = %r.platform,
        app_version = %r.app_version,
        seq = r.seq,
        events_compared = c.events_compared,
        chunk_only = c.chunk_only,
        stream_only_unexpected = c.stream_only_unexpected,
        stream_only_expected = c.stream_only_expected,
        predicted_skips = c.predicted_skips,
        verified_skips = c.verified_skips,
        false_skip = c.false_skip,
        live_rooms_checked = c.live_rooms_checked,
        live_missed = c.live_missed,
        skip_audited = c.skip_audited,
        skip_audit_new = c.skip_audit_new,
        skip_audit_failed = c.skip_audit_failed,
        seq_write_skipped_order = c.seq_write_skipped_order,
        reconnects = c.reconnects,
        snapshot_ms = c.snapshot_ms,
        closed_off = k.off,
        closed_not_enabled = k.not_enabled,
        closed_invalid = k.invalid,
        closed_too_many_rooms = k.too_many_rooms,
        closed_restricted = k.restricted,
        closed_auth_required = k.auth_required,
        closed_shadow_full = k.shadow_full,
        closed_full = k.full,
        closed_lagged = k.lagged,
        closed_closed = k.closed,
        closed_other = k.other,
        "user-stream shadow report"
    );
}

#[cfg(test)]
#[path = "user_stream_report_tests.rs"]
mod tests;
