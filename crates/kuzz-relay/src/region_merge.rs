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
//! Search (S4-3d) is merged by relevance: for page `p` of `n` hits each
//! part returns its best `p * n` hits with their scores (`ts_rank_cd`, the
//! same scale in every region), and the entry sorts them by score, then
//! `created_at` newest first, then `id`, the search query's own order, and
//! answers hits `(p - 1) * n` to `p * n`. A part also returns the hits it
//! hides from the reader, as score, time and id without the event, so the
//! entry pages over the same list a single region would. Parts return at
//! most [`SEARCH_DEPTH`] hits, and a page reaching past that depth is empty.
//! Profile search (kind 0, global-only) stays local until S5.
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

/// Search keys a merged search understands, besides [`PLAIN_KEYS`].
const SEARCH_KEYS: &[&str] = &[
    "search",
    "search_mode",
    "searchMode",
    "page",
    "search_page",
    "searchPage",
];

/// Filter key asking a region for its search hits with their scores. Only
/// honoured for a part of a merged search, never from a reader.
pub(crate) const SEARCH_RANKS_KEY: &str = "region_search_ranks";

/// The most hits one part of a merged search returns (the search page cap).
pub(crate) const SEARCH_DEPTH: usize = 500;

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
    /// For a search, each filter's 1-based page.
    pub search_pages: Option<Vec<usize>>,
}

/// The rooms a plan names in other regions, by region.
pub(crate) fn remote_rooms(plan: &Plan) -> Vec<(String, Vec<Uuid>)> {
    let mut by_region: Vec<(String, Vec<Uuid>)> = Vec::new();
    for part in plan.parts.iter().filter(|part| part.named) {
        let Some(region) = part.region.as_ref() else {
            continue;
        };
        let rooms = part.raw["#h"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|room| room.as_str()?.parse::<Uuid>().ok());
        let index = match by_region.iter().position(|(name, _)| name == region) {
            Some(index) => index,
            None => {
                by_region.push((region.clone(), Vec::new()));
                by_region.len() - 1
            }
        };
        for room in rooms {
            if !by_region[index].1.contains(&room) {
                by_region[index].1.push(room);
            }
        }
    }
    by_region
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
    let search = !filters.is_empty() && filters.iter().all(|filter| filter.search.is_some());
    if !state.config.region_link
        || filters.len() != raw.len()
        || !raw.iter().all(|raw| is_plain(raw, search))
    {
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
    if search {
        let limits: Vec<usize> = filters.iter().map(search_limit).collect();
        let pages: Vec<usize> = raw.iter().map(search_page).collect();
        for part in &mut parts {
            let depth = pages[part.filter]
                .saturating_mul(limits[part.filter])
                .min(SEARCH_DEPTH);
            part.raw = search_part(&part.raw, depth);
        }
        return Ok(Some(Plan {
            parts,
            limits,
            every_filter_named,
            search_pages: Some(pages),
        }));
    }
    let limits = filters.iter().map(event_limit).collect();
    Ok(Some(Plan {
        parts,
        limits,
        every_filter_named,
        search_pages: None,
    }))
}

/// A search filter's page size, as the HTTP search applies it.
fn search_limit(filter: &nostr::Filter) -> usize {
    filter.limit.unwrap_or(100).min(SEARCH_DEPTH)
}

/// A search filter's 1-based page.
fn search_page(raw: &Value) -> usize {
    ["page", "search_page", "searchPage"]
        .iter()
        .find_map(|key| raw.get(key).and_then(Value::as_u64))
        .and_then(|page| usize::try_from(page).ok())
        .filter(|&page| page > 0)
        .map_or(1, |page| page.min(SEARCH_PAGE_MAX))
}

/// The search query's own page cap.
const SEARCH_PAGE_MAX: usize = 1000;

/// A part of a merged search: its best `depth` hits, with scores.
fn search_part(raw: &Value, depth: usize) -> Value {
    let mut raw = raw.clone();
    if let Some(object) = raw.as_object_mut() {
        object.remove("search_page");
        object.remove("searchPage");
        object.insert("page".into(), json!(1));
        object.insert("limit".into(), json!(depth));
        object.insert(SEARCH_RANKS_KEY.into(), json!(true));
    }
    raw
}

/// Whether `raw` uses only keys a merged read (or, with `search`, a merged
/// search) understands.
fn is_plain(raw: &Value, search: bool) -> bool {
    raw.as_object().is_some_and(|object| {
        object.keys().all(|key| {
            key.starts_with('#')
                || PLAIN_KEYS.contains(&key.as_str())
                || (search && SEARCH_KEYS.contains(&key.as_str()))
        })
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
    if let Some(pages) = &plan.search_pages {
        return search(state, tenant, pubkey, plan, pages, reader).await;
    }
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
    if !plan.every_filter_named || plan.search_pages.is_some() {
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

/// Run a merged search: one call per part (a region's search hits cannot be
/// split back by filter), then merge by score.
async fn search(
    state: &Arc<AppState>,
    tenant: &TenantContext,
    pubkey: &nostr::PublicKey,
    plan: &Plan,
    pages: &[usize],
    reader: &EntryPrincipal,
) -> Result<Vec<Value>, QueryError> {
    let calls = plan.parts.iter().map(|part| async move {
        let filters = vec![part.raw.clone()];
        let answer = read_in(
            state,
            tenant,
            pubkey,
            part.region.as_deref(),
            ReadMode::Query,
            filters,
            reader,
        )
        .await
        .and_then(|answer| match answer.get("search_hits") {
            Some(Value::Array(hits)) => Ok(Value::Array(hits.clone())),
            _ => {
                tracing::warn!(target_region = ?part.region, "Malformed merged-search answer");
                Err(unreachable_error())
            }
        });
        PartAnswer {
            filter: part.filter,
            region: part.region.clone(),
            named: part.named,
            answer,
        }
    });
    let answers = futures_util::future::join_all(calls).await;
    merge_search(&plan.limits, pages, answers)
}

/// Merge search parts (each answer an array of `{"rank", "created_at",
/// "id", "event"}`, `event` absent for a hit hidden from the reader): per
/// filter the hits sorted by score, then the filter's page of them without
/// the hidden ones, then the filters in order with each event once. A page
/// reaching past [`SEARCH_DEPTH`] is empty. Failures as in [`merge_events`].
pub(crate) fn merge_search(
    limits: &[usize],
    pages: &[usize],
    answers: Vec<PartAnswer>,
) -> Result<Vec<Value>, QueryError> {
    let mut per_filter: Vec<Vec<Value>> = vec![Vec::new(); limits.len()];
    let mut answered = false;
    let mut refusal = None;
    for part in answers {
        match part.answer {
            Ok(Value::Array(hits)) if part.filter < per_filter.len() => {
                answered = true;
                per_filter[part.filter].extend(hits);
            }
            Ok(_) => {}
            Err(error) => {
                let failed = error.0.is_server_error() || error.0 == StatusCode::TOO_MANY_REQUESTS;
                tracing::warn!(target_region = ?part.region, status = %error.0, "Region part of a merged search failed");
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
    for ((mut hits, &limit), &page) in per_filter.into_iter().zip(limits).zip(pages) {
        if page.saturating_mul(limit) > SEARCH_DEPTH {
            continue;
        }
        hits.sort_by(|left, right| {
            let rank = |hit: &Value| hit.get("rank").and_then(Value::as_f64).unwrap_or(0.0);
            rank(right)
                .total_cmp(&rank(left))
                .then_with(|| event_order(left, right))
        });
        hits.dedup_by(|left, right| left.get("id") == right.get("id"));
        let start = (page - 1).saturating_mul(limit);
        for hit in hits.into_iter().skip(start).take(limit) {
            let Some(event) = hit.get("event").cloned() else {
                continue;
            };
            let id = event.get("id").and_then(Value::as_str).map(str::to_string);
            if id.is_none_or(|id| seen.insert(id)) {
                merged.push(event);
            }
        }
    }
    Ok(merged)
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
            &json!({"kinds": [9], "#h": ["x"], "until": 5, "before_id": "ab"}),
            false
        ));
        assert!(!is_plain(&json!({"kinds": [9], "search": "hi"}), false));
        assert!(is_plain(
            &json!({"kinds": [9], "search": "hi", "page": 2}),
            true
        ));
        assert!(!is_plain(&json!({"search": "hi", "top_level": true}), true));
        assert!(!is_plain(&json!({"#h": ["x"], "top_level": true}), false));
        assert!(!is_plain(&json!({"#h": ["x"], "after_seq": 3}), false));
        assert!(!is_plain(&json!([]), false));
    }

    #[test]
    fn search_parts_ask_for_every_page_up_to_this_one() {
        let raw = json!({"search": "hi", "searchPage": 3, "limit": 20});
        assert_eq!(search_page(&raw), 3);
        let part = search_part(&raw, 60);
        assert_eq!(part["page"], 1);
        assert_eq!(part["limit"], 60);
        assert_eq!(part[SEARCH_RANKS_KEY], true);
        assert!(part.get("searchPage").is_none());
        assert_eq!(search_limit(&nostr::Filter::new().search("hi")), 100);
    }

    #[test]
    fn search_merges_by_score_then_pages() {
        let hit = |at: u64, id: &str, rank: f64| {
            let mut hit = event(at, id);
            hit["rank"] = json!(rank);
            hit["event"] = event(at, id);
            hit
        };
        let here = json!([hit(10, "a", 0.9), hit(30, "c", 0.5)]);
        let jp = json!([hit(20, "b", 0.7), hit(40, "d", 0.5), hit(1, "e", 0.1)]);
        let answers = || {
            vec![
                answer(0, None, true, Ok(here.clone())),
                answer(0, Some("jp"), true, Ok(jp.clone())),
            ]
        };
        let first = merge_search(&[2], &[1], answers()).unwrap();
        assert_eq!(ids(&first), ["a", "b"]);
        // Equal scores: newer first.
        let second = merge_search(&[2], &[2], answers()).unwrap();
        assert_eq!(ids(&second), ["d", "c"]);
        let third = merge_search(&[2], &[3], answers()).unwrap();
        assert_eq!(ids(&third), ["e"]);
        // A hidden hit keeps its place: page 2 is the same as one region's.
        let mut hidden = hit(50, "f", 0.95);
        hidden.as_object_mut().unwrap().remove("event");
        let with_hidden = vec![
            answer(0, None, true, Ok(json!([hidden, hit(10, "a", 0.9)]))),
            answer(0, Some("jp"), true, Ok(jp.clone())),
        ];
        assert_eq!(ids(&merge_search(&[2], &[1], with_hidden).unwrap()), ["a"]);
        // Past the depth every part can return, a page is empty.
        assert!(merge_search(&[400], &[2], answers()).unwrap().is_empty());
        let down = answer(0, Some("sg"), true, Err(unreachable_error()));
        let error = merge_search(&[2], &[1], vec![down]).unwrap_err();
        assert_eq!(error.0, StatusCode::SERVICE_UNAVAILABLE);
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
