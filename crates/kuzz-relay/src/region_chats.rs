//! Chats and Inbox across regions (global regions S4-3b).
//!
//! `chat_list` and `inbox_summary` list the reader's rooms, and every region
//! lists only the rooms it serves. The entry asks every other region for the
//! same page and merges the rows by their shared keyset (`at_us`, then
//! `key`, newest first). Every region applies the same `before` cursor, so
//! the merged pages skip and repeat nothing, and the cursor keeps the shape
//! apps already send back. A region that cannot be reached is left out and
//! named in `unavailable_regions`; its rooms come back on the next refetch.
//!
//! Read positions and follows live with the room, so `mark_read` and
//! `thread_follow` are applied at the room's home ([`homes_of`] finds it);
//! `chat_import` goes to every region, which applies the items of its rooms.
//!
//! Only runs with the link on (`KUZZ_REGION_LINK`); with it off, or with one
//! region, nothing here is called.

use std::cmp::Ordering;

use axum::http::StatusCode;
use axum::Json;
use kuzz_core::tenant::TenantContext;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::region_link::EntryAuth;
use crate::region_read::{read_at_home, reader, ReadMode};
use crate::routing::RoomRoute;
use crate::state::AppState;

type QueryError = (StatusCode, Json<Value>);

/// What a request names whose home is wanted.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Target<'a> {
    /// A room.
    Room(Uuid),
    /// A thread, by its 32-byte root event id.
    Thread(&'a [u8]),
}

/// The other regions, when the link is on; empty otherwise.
pub(crate) async fn other_regions(state: &AppState) -> Vec<String> {
    if !state.config.region_link {
        return Vec::new();
    }
    match state.room_router.other_regions(&state.db).await {
        Ok(regions) => regions,
        Err(error) => {
            tracing::warn!(%error, "Region list lookup failed; serving this region only");
            Vec::new()
        }
    }
}

/// How long a list or lookup waits for one region before leaving it out.
const FAN_OUT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Ask each of `regions` to answer `filters` for `pubkey` (an HTTP `/query`
/// reader), all at once, waiting at most [`FAN_OUT_TIMEOUT`] for each.
/// Returns each region's answer, in `regions` order.
pub(crate) async fn ask_regions(
    state: &AppState,
    tenant: &TenantContext,
    pubkey: &nostr::PublicKey,
    regions: &[String],
    filters: &[Value],
) -> Vec<(String, Result<Value, QueryError>)> {
    let calls = regions.iter().map(|region| async move {
        let reader = reader(
            pubkey,
            EntryAuth::Nip98,
            &kuzz_auth::Scope::all_known(),
            None,
        );
        let answer = tokio::time::timeout(
            FAN_OUT_TIMEOUT,
            read_at_home(
                state,
                tenant,
                region,
                ReadMode::Query,
                filters.to_vec(),
                reader,
            ),
        )
        .await
        .unwrap_or_else(|_| {
            Err(crate::api::api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                crate::region_read::UNREACHABLE,
            ))
        });
        (region.clone(), answer)
    });
    futures_util::future::join_all(calls).await
}

/// `local` merged with the pages other regions answered for the same
/// `envelope` request (`chat_list` or `inbox_summary`): the newest `limit`
/// rows of all of them, and a cursor when any region has more.
pub(crate) fn merge_pages(
    envelope: &str,
    local: Value,
    answers: Vec<(String, Result<Value, QueryError>)>,
    limit: usize,
) -> Value {
    let mut pages = vec![local];
    let mut unavailable = Vec::new();
    for (region, answer) in answers {
        match answer {
            Ok(page) if page.get(envelope) == Some(&json!(1)) => pages.push(page),
            Ok(_) => unavailable.push(region),
            Err(error) => {
                tracing::warn!(target_region = %region, ?error, envelope, "Region left out of a merged list");
                unavailable.push(region);
            }
        }
    }
    let mut rows = Vec::new();
    let mut cursors = Vec::new();
    for page in pages {
        if let Some(next) = page.get("next").filter(|next| !next.is_null()) {
            cursors.push(next.clone());
        }
        if let Some(Value::Array(page_rows)) = page.get("rows") {
            rows.extend(page_rows.iter().cloned());
        }
    }
    rows.sort_by(newest_first);
    rows.dedup_by(|left, right| left["key"] == right["key"]);
    let more = rows.len() > limit || !cursors.is_empty();
    rows.truncate(limit);
    // Every region continues from one point: the newest of the last merged
    // row and each region's own cursor. A region whose page was filtered
    // (rows it may not show) stopped at its cursor; continuing past it would
    // skip its next rows. Merged rows older than that point come again on
    // the next page.
    let next = if more {
        let mut points: Vec<Value> = cursors;
        if let Some(last) = rows.last() {
            points.push(json!({ "at_us": last["at_us"], "key": last["key"] }));
        }
        points.into_iter().min_by(newest_first)
    } else {
        None
    };
    if let Some(next) = &next {
        rows.retain(|row| newest_first(row, next) != Ordering::Greater);
    }
    let next = next.unwrap_or(Value::Null);
    let mut merged = json!({ envelope: 1, "rows": rows, "next": next });
    if !unavailable.is_empty() {
        merged["unavailable_regions"] = json!(unavailable);
    }
    merged
}

/// Order of two rows (or cursors) by `(at_us, key)`, newest first.
fn newest_first(left: &Value, right: &Value) -> Ordering {
    let key = |row: &Value| {
        (
            row.get("at_us").and_then(Value::as_i64).unwrap_or(i64::MIN),
            row.get("key")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        )
    };
    key(right).cmp(&key(left))
}

/// The region that serves each target: `None` for this region, or when no
/// region knows the target (the ordinary refusal applies here).
///
/// A room's home comes from the room directory. A thread's room is found
/// from its root: here first, then by asking the other regions for the
/// roots the reader can see.
pub(crate) async fn homes_of(
    state: &AppState,
    tenant: &TenantContext,
    pubkey: &nostr::PublicKey,
    targets: &[Target<'_>],
) -> Result<Vec<Option<String>>, QueryError> {
    let mut homes = vec![None; targets.len()];
    let others = other_regions(state).await;
    if others.is_empty() {
        return Ok(homes);
    }
    let lookup_failed =
        |e: kuzz_db::DbError| crate::api::internal_error(&format!("room region lookup: {e}"));
    let roots: Vec<&[u8]> = targets
        .iter()
        .filter_map(|target| match target {
            Target::Thread(root) => Some(*root),
            Target::Room(_) => None,
        })
        .collect();
    let stored = if roots.is_empty() {
        Vec::new()
    } else {
        state
            .db
            .get_events_by_ids(tenant.community(), &roots)
            .await
            .map_err(lookup_failed)?
    };
    let mut unknown_roots = Vec::new();
    for (index, target) in targets.iter().enumerate() {
        let room = match target {
            Target::Room(room) => Some(*room),
            Target::Thread(root) => {
                let found = stored
                    .iter()
                    .find(|event| event.event.id.as_bytes() == *root);
                match found {
                    Some(event) => event.channel_id,
                    None => {
                        unknown_roots.push(index);
                        None
                    }
                }
            }
        };
        if let Some(room) = room {
            let route = state
                .room_router
                .route(&state.db, tenant.community(), room)
                .await
                .map_err(lookup_failed)?;
            if let RoomRoute::Elsewhere(home) = route {
                homes[index] = Some(home.region);
            }
        }
    }
    if unknown_roots.is_empty() {
        return Ok(homes);
    }
    let ids: Vec<String> = unknown_roots
        .iter()
        .filter_map(|&index| match targets[index] {
            Target::Thread(root) => Some(hex::encode(root)),
            Target::Room(_) => None,
        })
        .collect();
    let filters = [json!({ "ids": ids, "limit": ids.len() })];
    for (region, answer) in ask_regions(state, tenant, pubkey, &others, &filters).await {
        let Ok(Value::Array(events)) = answer else {
            continue;
        };
        for event in events {
            let Some(id) = event.get("id").and_then(Value::as_str) else {
                continue;
            };
            for &index in &unknown_roots {
                if let Target::Thread(root) = targets[index] {
                    if homes[index].is_none() && hex::encode(root) == id {
                        homes[index] = Some(region.clone());
                    }
                }
            }
        }
    }
    Ok(homes)
}

/// A home's refusal of one group of `mark_read` items, renumbered to the
/// item's index in the whole request (`mark_read item N: ...`).
pub(crate) fn renumber_refusal(mut error: QueryError, group: &[usize]) -> QueryError {
    const PREFIX: &str = "mark_read item ";
    let body = &mut error.1 .0;
    if let Some(message) = body.get("error").and_then(Value::as_str) {
        if let Some(rest) = message.strip_prefix(PREFIX) {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if let Some(&index) = digits.parse::<usize>().ok().and_then(|n| group.get(n)) {
                let renumbered = format!("{PREFIX}{index}{}", &rest[digits.len()..]);
                body["error"] = json!(renumbered);
            }
        }
    }
    error
}

/// One item result per request item from several regions' `chat_import`
/// answers: an applied result wins, then any refusal other than "not here"
/// (`not_member`, `unknown_thread`), then this region's own.
///
/// A region that could not be reached is named in `unavailable_regions`:
/// its rooms' items read as skipped, and the app may send the import again.
pub(crate) fn merge_import(
    local: Value,
    answers: Vec<(String, Result<Value, QueryError>)>,
) -> Value {
    let mut unavailable = Vec::new();
    let remote: Vec<Value> = answers
        .into_iter()
        .filter_map(|(region, answer)| match answer {
            Ok(answer) if answer.get("chat_import") == Some(&json!(1)) => Some(answer),
            other => {
                tracing::warn!(target_region = %region, ?other, "Region left out of a chat import");
                unavailable.push(region);
                None
            }
        })
        .collect();
    let mut merged = local;
    if !unavailable.is_empty() {
        merged["unavailable_regions"] = json!(unavailable);
    }
    for list in ["reads", "follows"] {
        let Some(Value::Array(items)) = merged.get_mut(list) else {
            continue;
        };
        for (index, item) in items.iter_mut().enumerate() {
            let candidates = remote
                .iter()
                .filter_map(|answer| answer.get(list)?.get(index));
            for candidate in candidates {
                if import_rank(candidate) > import_rank(item) {
                    *item = candidate.clone();
                }
            }
        }
    }
    merged
}

/// How much a region knew about an imported item.
fn import_rank(item: &Value) -> u8 {
    if item.get("applied") == Some(&json!(true)) {
        return 2;
    }
    match item.get("reason").and_then(Value::as_str) {
        Some("not_member" | "unknown_thread") | None => 0,
        Some(_) => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(at_us: i64, key: &str) -> Value {
        json!({ "at_us": at_us, "key": key })
    }

    fn page(rows: Vec<Value>, next: Value) -> Value {
        json!({ "chat_list": 1, "rows": rows, "next": next })
    }

    #[test]
    fn merged_pages_keep_the_newest_rows_and_continue_from_the_last() {
        let local = page(vec![row(90, "a"), row(50, "b")], Value::Null);
        let jp = page(vec![row(80, "c"), row(70, "d")], row(70, "d"));
        let merged = merge_pages("chat_list", local, vec![("jp".into(), Ok(jp))], 3);
        let keys: Vec<_> = merged["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["key"].as_str().unwrap())
            .collect();
        assert_eq!(keys, ["a", "c", "d"]);
        assert_eq!(merged["next"], row(70, "d"));
        assert!(merged.get("unavailable_regions").is_none());
    }

    #[test]
    fn a_last_page_everywhere_has_no_cursor() {
        let local = page(vec![row(10, "a")], Value::Null);
        let jp = page(vec![row(20, "b")], Value::Null);
        let merged = merge_pages("chat_list", local, vec![("jp".into(), Ok(jp))], 5);
        assert_eq!(merged["rows"].as_array().unwrap().len(), 2);
        assert!(merged["next"].is_null());
    }

    #[test]
    fn same_time_rows_order_by_key_and_empty_pages_keep_paging() {
        let local = page(vec![row(10, "a")], Value::Null);
        let jp = page(vec![row(10, "b")], Value::Null);
        let merged = merge_pages("chat_list", local, vec![("jp".into(), Ok(jp))], 1);
        assert_eq!(merged["rows"][0]["key"], "b");
        assert_eq!(merged["next"], row(10, "b"));

        let local = page(vec![], row(40, "x"));
        let jp = page(vec![], row(60, "y"));
        let merged = merge_pages("chat_list", local, vec![("jp".into(), Ok(jp))], 5);
        assert_eq!(merged["next"], row(60, "y"));
    }

    #[test]
    fn a_filtered_region_page_holds_the_cursor_back() {
        // jp's whole page was filtered, its cursor at 90; kr's rows are older.
        let jp = page(vec![], row(90, "j"));
        let kr = page(vec![row(80, "a"), row(70, "b")], Value::Null);
        let merged = merge_pages("chat_list", kr, vec![("jp".into(), Ok(jp))], 2);
        assert_eq!(merged["next"], row(90, "j"));
        assert!(merged["rows"].as_array().unwrap().is_empty());

        // A cursor older than the last row changes nothing.
        let jp = page(vec![row(95, "c")], row(95, "c"));
        let kr = page(vec![row(99, "a"), row(97, "b")], row(97, "b"));
        let merged = merge_pages("chat_list", kr, vec![("jp".into(), Ok(jp))], 2);
        assert_eq!(merged["next"], row(97, "b"));
        assert_eq!(merged["rows"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn an_unreachable_region_is_named_and_left_out() {
        let local = page(vec![row(10, "a")], Value::Null);
        let down = Err(crate::api::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "down",
        ));
        let wrong = Ok(json!({ "inbox_summary": 1, "rows": [row(99, "z")] }));
        let merged = merge_pages(
            "chat_list",
            local,
            vec![("jp".into(), down), ("sg".into(), wrong)],
            5,
        );
        assert_eq!(merged["rows"].as_array().unwrap().len(), 1);
        assert_eq!(merged["unavailable_regions"], json!(["jp", "sg"]));
    }

    #[test]
    fn refusals_name_the_item_in_the_whole_request() {
        let refusal = crate::api::api_error(
            StatusCode::FORBIDDEN,
            "mark_read item 1: not a member of that room",
        );
        let renumbered = renumber_refusal(refusal, &[2, 5]);
        assert_eq!(
            renumbered.1 .0["error"],
            "mark_read item 5: not a member of that room"
        );
        let other = crate::api::api_error(StatusCode::FORBIDDEN, "mark_read: nope");
        assert_eq!(
            renumber_refusal(other, &[3]).1 .0["error"],
            "mark_read: nope"
        );
    }

    #[test]
    fn imports_keep_what_the_room_home_applied() {
        let local = json!({ "chat_import": 1,
            "reads": [{"channel": "a", "applied": false, "reason": "not_member"},
                      {"channel": "b", "applied": true, "reason": null}],
            "follows": [{"root": "r", "applied": false, "reason": "unknown_thread"}] });
        let kr = json!({ "chat_import": 1,
            "reads": [{"channel": "a", "applied": true, "reason": null, "read_seq": 4},
                      {"channel": "b", "applied": false, "reason": "not_member"}],
            "follows": [{"root": "r", "applied": false, "reason": "already_set"}] });
        let down = Err(crate::api::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "down",
        ));
        let merged = merge_import(local, vec![("kr".into(), Ok(kr)), ("sg".into(), down)]);
        assert_eq!(merged["unavailable_regions"], json!(["sg"]));
        assert_eq!(merged["reads"][0]["read_seq"], 4);
        assert_eq!(merged["reads"][1]["applied"], true);
        assert_eq!(merged["follows"][0]["reason"], "already_set");
    }
}
