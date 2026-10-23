//! Media cleanup across regions (global regions S6-2).
//!
//! Regions share one bucket, but each region's database knows only the
//! messages and uploads of its own rooms. The cleanup runs in the primary
//! region and asks every other region which of the files it is about to
//! remove are in use there (`POST /internal/region/media-in-use`). A region
//! that does not answer keeps every file in question: nothing is removed on
//! a missing answer.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::Json;
use chrono::{DateTime, Utc};
use kuzz_core::CommunityId;
use kuzz_db::media_access::MAX_CLEANUP_HASHES_PER_CALL;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::state::AppState;

/// The route each region answers on.
pub const MEDIA_IN_USE_PATH: &str = "/internal/region/media-in-use";

/// One region's answer may take as long as its database check.
const ASK_TIMEOUT: Duration = Duration::from_secs(150);

/// Which of `hashes` are in use, counting uploads and deletions since
/// `grace_cutoff` (unix seconds) as use.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct InUseQuestion {
    hashes: Vec<String>,
    grace_cutoff: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct InUseAnswer {
    in_use: Vec<String>,
}

/// The files among `hashes` that another region's rooms use in `community`.
/// `Err` when any other region does not answer: the caller keeps the files.
/// Empty without asking when there is no other region.
pub async fn in_use_elsewhere(
    state: &AppState,
    community: CommunityId,
    hashes: &[String],
    grace_cutoff: DateTime<Utc>,
) -> Result<HashSet<String>, String> {
    let mut in_use = HashSet::new();
    if hashes.is_empty() {
        return Ok(in_use);
    }
    let regions = state.db.list_regions().await.map_err(|e| e.to_string())?;
    let question = serde_json::to_value(InUseQuestion {
        hashes: hashes.to_vec(),
        grace_cutoff: grace_cutoff.timestamp(),
    })
    .map_err(|e| e.to_string())?;
    for region in regions
        .iter()
        .filter(|region| region.id != state.config.region_id)
    {
        let answer = tokio::time::timeout(
            ASK_TIMEOUT,
            crate::region_link::call(
                state,
                &region.id,
                *community.as_uuid(),
                MEDIA_IN_USE_PATH,
                &question,
                None,
                None,
            ),
        )
        .await;
        let answer = match answer {
            Ok(Ok(answer)) => serde_json::from_value::<InUseAnswer>(answer)
                .map_err(|e| format!("region {} answered badly: {e}", region.id))?,
            Ok(Err(error)) => return Err(format!("region {}: {error:?}", region.id)),
            Err(_) => return Err(format!("region {} did not answer in time", region.id)),
        };
        // Only hashes that were asked about count.
        in_use.extend(answer.in_use.into_iter().filter(|sha| hashes.contains(sha)));
    }
    Ok(in_use)
}

fn answer_error(status: StatusCode, kind: &str, message: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": kind, "message": message })))
}

/// `POST /internal/region/media-in-use`: which of the files are in use in
/// this region's rooms, for the call's community.
pub async fn media_in_use(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> (StatusCode, Json<Value>) {
    let call = match crate::region_link::admit(&state, &Method::POST, &uri, &headers, &body).await {
        Ok(call) => call,
        Err(refusal) => {
            metrics::counter!("kuzz_region_link_calls_refused_total", "reason" => refusal.label())
                .increment(1);
            tracing::warn!(reason = refusal.label(), "Refused a media check");
            return answer_error(StatusCode::UNAUTHORIZED, "refused", "region link refused");
        }
    };
    let Some((hashes, cutoff)) = parse(&body) else {
        return answer_error(StatusCode::BAD_REQUEST, "malformed", "malformed question");
    };
    match state
        .db
        .media_hashes_in_use(call.tenant.community(), &hashes, cutoff)
        .await
    {
        Ok(in_use) => (StatusCode::OK, Json(json!(InUseAnswer { in_use }))),
        Err(error) => {
            tracing::warn!(%error, "A region's media check failed");
            answer_error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "try again")
        }
    }
}

/// A well-formed question: at most one cleanup batch of lowercase SHA-256
/// hashes and a valid time.
fn parse(body: &[u8]) -> Option<(Vec<String>, DateTime<Utc>)> {
    let question = serde_json::from_slice::<InUseQuestion>(body).ok()?;
    let valid = question.hashes.len() <= MAX_CLEANUP_HASHES_PER_CALL
        && question
            .hashes
            .iter()
            .all(|sha| sha.len() == 64 && sha.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    let cutoff = DateTime::from_timestamp(question.grace_cutoff, 0)?;
    valid.then_some((question.hashes, cutoff))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions_carry_hashes_and_a_time() {
        let sha = "ab".repeat(32);
        let body = serde_json::to_vec(&InUseQuestion {
            hashes: vec![sha.clone()],
            grace_cutoff: 1_700_000_000,
        })
        .unwrap();
        let (hashes, cutoff) = parse(&body).expect("valid");
        assert_eq!(hashes, [sha]);
        assert_eq!(cutoff.timestamp(), 1_700_000_000);

        let bad = |hashes: Vec<String>| {
            parse(
                &serde_json::to_vec(&InUseQuestion {
                    hashes,
                    grace_cutoff: 1,
                })
                .unwrap(),
            )
        };
        assert!(bad(vec!["AB".repeat(32)]).is_none());
        assert!(bad(vec!["ab".into()]).is_none());
        assert!(bad(vec!["ab".repeat(32); MAX_CLEANUP_HASHES_PER_CALL + 1]).is_none());
        assert!(parse(b"{}").is_none());
    }
}
