//! Reads that span regions (global regions S4-3c).
//!
//! A read naming rooms of several regions, or of this region and another,
//! is split by home: each region gets the filter with only its own rooms in
//! `#h`, and the answers are merged per filter in the events' own order
//! (`created_at` newest first, then `id`, as the event query sorts) and cut
//! to the filter's limit. Every part applies the same `until` and
//! `before_id` cursor, so pages merged this way skip and repeat nothing.
//!
//! A filter that names no room can match events of any room the reader is
//! in, so it goes to every region, unless it asks only for global kinds
//! (profiles, lists), which stay local until S5 copies them everywhere.
//!
//! `COUNT` adds up each region's count when every filter names rooms: a room
//! lives in one region, so no event is counted twice. A `COUNT` without
//! rooms stays local.
//!
//! Each region gets one call carrying all its filters; its events are
//! assigned back to the filters they match.
//!
//! A region that refuses its part (for example "not a member") is left out,
//! the way a local read omits rooms the reader may not open. A part naming
//! rooms that fails otherwise (unreachable, timed out, rate-limited) fails
//! the read: answering without those rooms would let the app page past
//! them. A filter without rooms leaves an unreachable region out, so one
//! region being down does not break every such read. Only plain filters are
//! merged: search (S4-3d), order-number pages, thread windows and the other
//! `/query` modes keep their single-region answers.
//!
//! Only runs with the link on (`KUZZ_REGION_LINK`); with it off, or with one
//! region, [`plan`] returns `None` and nothing changes.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use axum::http::StatusCode;
use axum::Json;
use kuzz_core::tenant::TenantContext;
use kuzz_core::CommunityId;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::region_link::EntryPrincipal;
use crate::region_read::{read_at_home, ReadMode, UNREACHABLE};
use crate::state::AppState;

type QueryError = (StatusCode, Json<Value>);

/// Filter keys a merged read understands. Any other key selects a `/query`
/// mode with its own order, which is not merged.
const PLAIN_KEYS: &[&str] = &[
    "ids",
    "authors",
    "kinds",
    "since",
    "until",
    "limit",
    "before_id",
];

/// One region's share of one filter. `region` is `None` for this region.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Part {
    pub filter: usize,
    pub region: Option<String>,
    pub raw: Value,
    /// Whether the filter names its rooms (a failure here fails the read).
    pub named: bool,
}

/// How a read is split across regions.
#[derive(Debug)]
pub(crate) struct Plan {
    pub parts: Vec<Part>,
    /// Each filter's limit, as the event query applies it.
    pub limits: Vec<usize>,
    /// Whether every filter names its rooms (a `COUNT` can be summed).
    pub every_filter_named: bool,
}

/// How `raw` filters (`filters` as `/query` JSON) are split across regions,
/// or `None` when the read stays here: the link is off, there is one region,
/// a filter is not plain, or no part goes to another region.
pub(crate) async fn plan(
    state: &AppState,
    community: CommunityId,
    filters: &[nostr::Filter],
    raw: &[Value],
) -> Result<Option<Plan>, kuzz_db::DbError> {
    if !state.config.region_link || filters.len() != raw.len() || !raw.iter().all(is_plain) {
        return Ok(None);
    }
    let others = state.room_router.other_regions(&state.db).await?;
    if others.is_empty() {
        return Ok(None);
    }
    let h = nostr::SingleLetterTag::lowercase(nostr::Alphabet::H);
    let mut parts = Vec::new();
    let mut every_filter_named = true;
    for (idx, (filter, raw)) in filters.iter().zip(raw).enumerate() {
        let part = |region: Option<String>, raw: Value, named: bool| Part {
            filter: idx,
            region,
            raw,
            named,
        };
        let Some(values) = filter.generic_tags.get(&h) else {
            every_filter_named = false;
            parts.push(part(None, raw.clone(), false));
            if let Some(remote) = room_kinds_only(filter, raw) {
                parts.extend(
                    others
                        .iter()
                        .map(|region| part(Some(region.clone()), remote.clone(), false)),
                );
            }
            continue;
        };
        let rooms: Vec<Uuid> = values.iter().filter_map(|v| v.parse().ok()).collect();
        if rooms.is_empty() {
            parts.push(part(None, raw.clone(), true));
            continue;
        }
        let split = state.room_router.split(&state.db, community, rooms).await?;
        if !split.here.is_empty() {
            parts.push(part(None, with_rooms(raw, &split.here), true));
        }
        for (region, rooms) in split.elsewhere {
            parts.push(part(Some(region), with_rooms(raw, &rooms), true));
        }
    }
    if parts.iter().all(|part| part.region.is_none()) {
        return Ok(None);
    }
    let limits = filters.iter().map(event_limit).collect();
    Ok(Some(Plan {
        parts,
        limits,
        every_filter_named,
    }))
}

/// Whether `raw` uses only keys a merged read understands.
fn is_plain(raw: &Value) -> bool {
    raw.as_object().is_some_and(|object| {
        object
            .keys()
            .all(|key| key.starts_with('#') || PLAIN_KEYS.contains(&key.as_str()))
    })
}

/// A room-less filter as other regions get it: without its global-only
/// kinds, whose copies elsewhere may be stale (S5 copies them), or `None`
/// when it asks only for global-only kinds and stays local.
fn room_kinds_only(filter: &nostr::Filter, raw: &Value) -> Option<Value> {
    let Some(kinds) = filter.kinds.as_ref().filter(|kinds| !kinds.is_empty()) else {
        return Some(raw.clone());
    };
    let room_kinds: Vec<u16> = kinds
        .iter()
        .map(|kind| kind.as_u16())
        .filter(|&kind| !crate::handlers::ingest::is_global_only_kind(u32::from(kind)))
        .collect();
    if room_kinds.is_empty() {
        return None;
    }
    let mut raw = raw.clone();
    raw["kinds"] = json!(room_kinds);
    Some(raw)
}

/// `raw` with `#h` naming only `rooms`.
fn with_rooms(raw: &Value, rooms: &[Uuid]) -> Value {
    let mut raw = raw.clone();
    raw["#h"] = json!(rooms.iter().map(Uuid::to_string).collect::<Vec<_>>());
    raw
}

/// The page size the event query uses for `filter`.
fn event_limit(filter: &nostr::Filter) -> usize {
    let max = usize::try_from(kuzz_db::DEFAULT_MAX_PAGE_LIMIT).unwrap_or(usize::MAX);
    filter.limit.map_or(max, |limit| limit.min(max))
}

/// The parts of `plan`, grouped by region (this region is `None`).
fn by_region(plan: &Plan) -> BTreeMap<Option<&str>, Vec<&Part>> {
    let mut groups: BTreeMap<Option<&str>, Vec<&Part>> = BTreeMap::new();
    for part in &plan.parts {
        groups.entry(part.region.as_deref()).or_default().push(part);
    }
    groups
}

/// Run `filters` in `region`: here through the ordinary path (already
/// admitted), elsewhere at the home for `reader`, bounded by the fan-out wait.
async fn read_in(
    state: &Arc<AppState>,
    tenant: &TenantContext,
    pubkey: &nostr::PublicKey,
    region: Option<&str>,
    mode: ReadMode,
    filters: Vec<Value>,
    reader: &EntryPrincipal,
) -> Result<Value, QueryError> {
    match region {
        None => {
            let body = Value::Array(filters).to_string();
            crate::api::bridge::read_part_here(state, tenant, body.as_bytes(), *pubkey, mode)
                .await
                .map(|Json(answer)| answer)
        }
        Some(region) => tokio::time::timeout(
            crate::region_chats::FAN_OUT_TIMEOUT,
            read_at_home(state, tenant, region, mode, filters, reader.clone()),
        )
        .await
        .unwrap_or_else(|_| Err(unreachable_error())),
    }
}

/// Run `plan` with one call per region, all at once, and merge the events.
pub(crate) async fn query(
    state: &Arc<AppState>,
    tenant: &TenantContext,
    pubkey: &nostr::PublicKey,
    plan: &Plan,
    reader: &EntryPrincipal,
) -> Result<Vec<Value>, QueryError> {
    let calls = by_region(plan)
        .into_iter()
        .map(|(region, parts)| async move {
            let filters = parts.iter().map(|part| part.raw.clone()).collect();
            let answer = read_in(
                state,
                tenant,
                pubkey,
                region,
                ReadMode::Query,
                filters,
                reader,
            )
            .await;
            split_answer(&parts, answer)
        });
    let answers = futures_util::future::join_all(calls).await;
    merge_events(&plan.limits, answers.into_iter().flatten().collect())
}

/// Count `plan` at every region it spans and add the counts up. `None` when
/// a filter names no rooms: such a count stays local. Any failed region
/// fails the count, which must be exact.
pub(crate) async fn count(
    state: &Arc<AppState>,
    tenant: &TenantContext,
    pubkey: &nostr::PublicKey,
    plan: &Plan,
    reader: &EntryPrincipal,
) -> Option<Result<u64, QueryError>> {
    if !plan.every_filter_named {
        return None;
    }
    // One call per region: its rooms' filters are counted together, as a
    // local COUNT counts an event matching several filters once.
    let calls = by_region(plan)
        .into_iter()
        .map(|(region, parts)| async move {
            let filters = parts.iter().map(|part| part.raw.clone()).collect();
            let answer = read_in(
                state,
                tenant,
                pubkey,
                region,
                ReadMode::Count,
                filters,
                reader,
            )
            .await;
            answer.and_then(|answer| {
                answer.get("count").and_then(Value::as_u64).ok_or_else(|| {
                    tracing::warn!(target_region = ?region, "Malformed count answer");
                    unreachable_error()
                })
            })
        });
    let counts = futures_util::future::join_all(calls).await;
    Some(counts.into_iter().sum())
}

fn unreachable_error() -> QueryError {
    crate::api::api_error(StatusCode::SERVICE_UNAVAILABLE, UNREACHABLE)
}

/// One part's events (or failure), for [`merge_events`].
pub(crate) struct PartAnswer {
    pub filter: usize,
    pub region: Option<String>,
    pub named: bool,
    pub answer: Result<Value, QueryError>,
}

/// One region's answer to its `parts`, split back into each part's events:
/// an event belongs to every part whose filter it matches.
fn split_answer(parts: &[&Part], answer: Result<Value, QueryError>) -> Vec<PartAnswer> {
    let answer_for = |part: &Part, answer: Result<Value, QueryError>| PartAnswer {
        filter: part.filter,
        region: part.region.clone(),
        named: part.named,
        answer,
    };
    let events = match answer {
        Ok(Value::Array(events)) => events,
        Ok(_) => {
            tracing::warn!("Malformed merged-read answer");
            return parts
                .iter()
                .map(|part| answer_for(part, Err(unreachable_error())))
                .collect();
        }
        Err(error) => {
            return parts
                .iter()
                .map(|part| answer_for(part, Err(error.clone())))
                .collect();
        }
    };
    if let [part] = parts {
        return vec![answer_for(part, Ok(Value::Array(events)))];
    }
    let parsed: Vec<Option<nostr::Event>> = events
        .iter()
        .map(|event| serde_json::from_value(event.clone()).ok())
        .collect();
    parts
        .iter()
        .map(|part| {
            let matched = events
                .iter()
                .zip(&parsed)
                .filter(|(_, event)| {
                    event
                        .as_ref()
                        .is_some_and(|event| part_matches(part, event))
                })
                .map(|(value, _)| value.clone())
                .collect();
            answer_for(part, Ok(Value::Array(matched)))
        })
        .collect()
}

/// Whether `event` matches `part`'s filter, including its `before_id`
/// tie-break at `until` (the event query keeps `id > before_id` there).
fn part_matches(part: &Part, event: &nostr::Event) -> bool {
    let Ok(filter) = serde_json::from_value::<nostr::Filter>(part.raw.clone()) else {
        return false;
    };
    if !filter.match_event(event, nostr::filter::MatchEventOptions::default()) {
        return false;
    }
    match (
        filter.until,
        part.raw.get("before_id").and_then(Value::as_str),
    ) {
        (Some(until), Some(before_id)) if event.created_at == until => {
            event.id.to_hex().as_str() > before_id
        }
        _ => true,
    }
}

/// Merge each part's events into the read's answer: per filter the newest
/// events up to its limit, then the filters in order with each event once.
///
/// A refused part is left out. A part naming rooms that fails otherwise
/// fails the read. A room-less part that fails is left out. When no part
/// answered, the first refusal (or "unreachable") is the read's answer.
pub(crate) fn merge_events(
    limits: &[usize],
    answers: Vec<PartAnswer>,
) -> Result<Vec<Value>, QueryError> {
    let mut per_filter: Vec<Vec<Value>> = vec![Vec::new(); limits.len()];
    let mut answered = false;
    let mut refusal = None;
    for part in answers {
        let region = part.region.as_deref().unwrap_or("here");
        match part.answer {
            Ok(Value::Array(events)) if part.filter < per_filter.len() => {
                answered = true;
                per_filter[part.filter].extend(events);
            }
            Ok(_) => {
                tracing::warn!(target_region = region, "Malformed merged-read answer");
            }
            Err(error) => {
                let failed = error.0.is_server_error() || error.0 == StatusCode::TOO_MANY_REQUESTS;
                tracing::warn!(target_region = region, status = %error.0, "Region part of a merged read failed");
                metrics::counter!(
                    "kuzz_region_merged_read_parts_failed_total",
                    "named" => if part.named { "true" } else { "false" }
                )
                .increment(1);
                if failed && part.named {
                    return Err(error);
                }
                if !failed && refusal.is_none() {
                    refusal = Some(error);
                }
            }
        }
    }
    if !answered {
        return Err(refusal.unwrap_or_else(unreachable_error));
    }
    let mut seen = HashSet::new();
    let mut merged = Vec::new();
    for (mut events, &limit) in per_filter.into_iter().zip(limits) {
        events.sort_by(event_order);
        events.dedup_by(|left, right| left.get("id") == right.get("id"));
        events.truncate(limit);
        for event in events {
            let id = event.get("id").and_then(Value::as_str).map(str::to_string);
            if id.is_none_or(|id| seen.insert(id)) {
                merged.push(event);
            }
        }
    }
    Ok(merged)
}

/// The event query's order: `created_at` newest first, then `id` ascending.
fn event_order(left: &Value, right: &Value) -> Ordering {
    let created_at = |event: &Value| event.get("created_at").and_then(Value::as_u64);
    let id = |event: &Value| {
        event
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    created_at(right)
        .cmp(&created_at(left))
        .then_with(|| id(left).cmp(&id(right)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(created_at: u64, id: &str) -> Value {
        json!({ "id": id.repeat(64 / id.len()), "created_at": created_at })
    }

    fn ids(events: &[Value]) -> Vec<String> {
        events
            .iter()
            .map(|event| event["id"].as_str().unwrap()[..1].to_string())
            .collect()
    }

    fn answer(
        filter: usize,
        region: Option<&str>,
        named: bool,
        answer: Result<Value, QueryError>,
    ) -> PartAnswer {
        PartAnswer {
            filter,
            region: region.map(str::to_string),
            named,
            answer,
        }
    }

    #[test]
    fn parts_merge_newest_first_cut_to_the_limit() {
        let here = json!([event(90, "a"), event(70, "c")]);
        let jp = json!([event(80, "b"), event(70, "0"), event(60, "d")]);
        let merged = merge_events(
            &[3],
            vec![
                answer(0, None, true, Ok(here)),
                answer(0, Some("jp"), true, Ok(jp)),
            ],
        )
        .unwrap();
        // Ties at one second sort by id ascending, as the event query does.
        assert_eq!(ids(&merged), ["a", "b", "0"]);
    }

    #[test]
    fn filters_keep_their_own_limits_and_each_event_once() {
        let first = json!([event(90, "a"), event(80, "b")]);
        let second = json!([event(80, "b"), event(70, "c")]);
        let merged = merge_events(
            &[1, 5],
            vec![
                answer(0, None, true, Ok(first)),
                answer(1, Some("jp"), true, Ok(second)),
            ],
        )
        .unwrap();
        assert_eq!(ids(&merged), ["a", "b", "c"]);
    }

    #[test]
    fn refusals_are_left_out_and_a_named_failure_fails_the_read() {
        let down = || Err(unreachable_error());
        let refused = || {
            Err(crate::api::api_error(
                StatusCode::FORBIDDEN,
                "restricted: not a channel member",
            ))
        };
        let jp = || Ok(json!([event(1, "a")]));
        // A refusal is left out; so is an unreachable region of a room-less filter.
        let merged = merge_events(
            &[5],
            vec![
                answer(0, None, true, refused()),
                answer(0, Some("jp"), true, jp()),
                answer(1, Some("sg"), false, down()),
            ],
        )
        .unwrap();
        assert_eq!(ids(&merged), ["a"]);
        // An unreachable region holding named rooms fails the read.
        let error = merge_events(
            &[5],
            vec![
                answer(0, Some("jp"), true, jp()),
                answer(0, Some("sg"), true, down()),
            ],
        )
        .unwrap_err();
        assert_eq!(error.0, StatusCode::SERVICE_UNAVAILABLE);
        // With nothing answered, the refusal is the answer.
        let error = merge_events(
            &[5],
            vec![
                answer(0, Some("sg"), false, down()),
                answer(0, None, true, refused()),
            ],
        )
        .unwrap_err();
        assert_eq!(error.0, StatusCode::FORBIDDEN);
    }

    #[test]
    fn one_call_per_region_is_split_back_by_filter() {
        let keys = nostr::Keys::generate();
        let (a, b) = (Uuid::new_v4().to_string(), Uuid::new_v4().to_string());
        let in_room = |room: &str, at: u64| {
            nostr::EventBuilder::new(nostr::Kind::Custom(9), "x")
                .tags([nostr::Tag::custom(nostr::TagKind::h(), [room])])
                .custom_created_at(nostr::Timestamp::from(at))
                .sign_with_keys(&keys)
                .unwrap()
        };
        let (first, second) = (in_room(&a, 10), in_room(&b, 20));
        let part = |filter: usize, raw: Value| Part {
            filter,
            region: Some("jp".into()),
            raw,
            named: true,
        };
        let parts = [
            part(0, json!({"kinds": [9], "#h": [a]})),
            part(1, json!({"kinds": [9], "#h": [b]})),
        ];
        let answer = Ok(json!([first, second]));
        let split = split_answer(&parts.iter().collect::<Vec<_>>(), answer);
        let ids = |idx: usize| {
            split[idx]
                .answer
                .as_ref()
                .unwrap()
                .as_array()
                .unwrap()
                .len()
        };
        assert_eq!((ids(0), ids(1)), (1, 1));

        // The before_id tie-break at `until` is kept.
        let at = part(
            0,
            json!({"kinds": [9], "#h": [a], "until": 10, "before_id": first.id.to_hex()}),
        );
        assert!(!part_matches(&at, &first));
        let before = part(
            0,
            json!({"kinds": [9], "#h": [a], "until": 10, "before_id": "00"}),
        );
        assert!(part_matches(&before, &first));
    }

    #[test]
    fn only_plain_filters_merge() {
        assert!(is_plain(
            &json!({"kinds": [9], "#h": ["x"], "until": 5, "before_id": "ab"})
        ));
        assert!(!is_plain(&json!({"kinds": [9], "search": "hi"})));
        assert!(!is_plain(&json!({"#h": ["x"], "top_level": true})));
        assert!(!is_plain(&json!({"#h": ["x"], "after_seq": 3})));
        assert!(!is_plain(&json!([])));
    }

    #[test]
    fn global_only_kinds_stay_local() {
        let profiles = nostr::Filter::new().kind(nostr::Kind::Metadata);
        assert_eq!(room_kinds_only(&profiles, &json!({"kinds": [0]})), None);
        let both = nostr::Filter::new().kinds([nostr::Kind::Metadata, nostr::Kind::Custom(9)]);
        assert_eq!(
            room_kinds_only(&both, &json!({"kinds": [0, 9]})),
            Some(json!({"kinds": [9]}))
        );
        let any = json!({"authors": ["ab"]});
        assert_eq!(room_kinds_only(&nostr::Filter::new(), &any), Some(any));
    }

    #[test]
    fn a_part_names_only_its_rooms() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let raw = json!({"kinds": [9], "#h": [a.to_string(), b.to_string()], "limit": 10});
        let part = with_rooms(&raw, &[b]);
        assert_eq!(part["#h"], json!([b.to_string()]));
        assert_eq!(part["limit"], 10);
        assert_eq!(
            event_limit(&nostr::Filter::new().limit(10_000_000)),
            usize::try_from(kuzz_db::DEFAULT_MAX_PAGE_LIMIT).unwrap()
        );
    }
}
