//! Store skip for the user stream (docs/message-sync.md#store-skip): which
//! frames of a socket's `$joined` subscription the store keeps. The verdict
//! is `pending` until the stream EOSE has validated the whole snapshot (the
//! same checks as the app's `part1` cases), then `keep` (live, gated) or
//! `skip` (shadow, any refusal). Frames seen while pending are dropped.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

/// The relay-signed snapshot part. Never stored.
pub(crate) const USER_STREAM_KIND: u64 = 24220;
/// The `#h` value naming every room of the account. Never a room id.
pub(crate) const USER_STREAM_ROOM: &str = "$joined";
/// The frozen v1 kinds the snapshot must announce (compared as a set).
pub(crate) const USER_STREAM_KINDS: [u64; 17] = [
    5, 7, 9, 9005, 39005, 40001, 40002, 40003, 40008, 40009, 40099, 45001, 45003, 48100, 48101,
    48102, 48103,
];
const MAX_EPOCH_CHARS: usize = 64;
const MAX_SEQ: u64 = (1 << 53) - 1;

/// Reads the snapshot parts of one stream REQ in arrival order.
#[derive(Default)]
pub(crate) struct SnapshotCheck {
    refused: bool,
    mode: Option<String>,
    announced: u64,
    received: u64,
    rooms: HashSet<String>,
}

impl SnapshotCheck {
    /// A frame on the stream sub before EOSE.
    pub(crate) fn feed(&mut self, event: &Value) {
        if self.refused || event.get("kind").and_then(Value::as_u64) != Some(USER_STREAM_KIND) {
            return;
        }
        let content = event
            .get("content")
            .and_then(Value::as_str)
            .and_then(|content| serde_json::from_str::<Value>(content).ok());
        if !self.accept(content) {
            self.refused = true;
        }
    }

    fn accept(&mut self, content: Option<Value>) -> bool {
        let Some(Value::Object(part)) = content else {
            return false;
        };
        if self.mode.is_none() {
            let mode = part.get("mode").and_then(Value::as_str);
            let epoch = part.get("epoch").and_then(Value::as_str);
            let parts = part.get("parts").and_then(Value::as_u64);
            let ok = part.get("v").and_then(Value::as_u64) == Some(1)
                && part.get("v").is_some_and(Value::is_u64)
                && part.get("op").and_then(Value::as_str) == Some("snapshot")
                && part.get("part").and_then(Value::as_u64) == Some(1)
                && part.get("part").is_some_and(Value::is_u64)
                && parts.is_some_and(|parts| parts >= 1)
                && matches!(mode, Some("shadow" | "live" | "gated"))
                && epoch.is_some_and(|epoch| {
                    !epoch.is_empty() && epoch.encode_utf16().count() <= MAX_EPOCH_CHARS
                })
                && same_kinds(part.get("kinds"));
            if !ok {
                return false;
            }
            self.mode = mode.map(str::to_owned);
            self.announced = parts.unwrap_or(0);
        } else {
            if self.received >= self.announced {
                return false;
            }
            if let Some(number) = part.get("part") {
                if !number.is_u64() || number.as_u64() != Some(self.received + 1) {
                    return false;
                }
            }
        }
        let Some(Value::Object(rooms)) = part.get("rooms") else {
            return false;
        };
        for (room, seq) in rooms {
            if !seq.as_u64().is_some_and(|seq| seq <= MAX_SEQ) || !seq.is_u64() {
                return false;
            }
            if !self.rooms.insert(room.clone()) {
                return false;
            }
        }
        self.received += 1;
        true
    }

    /// At the stream EOSE: the accepted mode, `None` when refused.
    pub(crate) fn eose(&self) -> Option<&str> {
        if self.refused || self.received != self.announced {
            return None;
        }
        self.mode.as_deref()
    }
}

fn same_kinds(kinds: Option<&Value>) -> bool {
    let Some(Value::Array(kinds)) = kinds else {
        return false;
    };
    let mut set = HashSet::new();
    for kind in kinds {
        let Some(kind) = kind.as_u64().filter(|_| kind.is_u64()) else {
            return false;
        };
        set.insert(kind);
    }
    set.len() == USER_STREAM_KINDS.len() && USER_STREAM_KINDS.iter().all(|kind| set.contains(kind))
}

enum Verdict {
    Pending(SnapshotCheck),
    Keep,
    Skip,
}

/// The stream subscriptions of one socket and their store verdicts.
#[derive(Default)]
pub(crate) struct StreamGate {
    subs: HashMap<String, Verdict>,
}

/// The parts of a client or relay frame: label and the array.
fn parts(frame: &str) -> Option<Vec<Value>> {
    match serde_json::from_str::<Value>(frame) {
        Ok(Value::Array(parts)) => Some(parts),
        _ => None,
    }
}

/// The sub id of a relay frame `["<label>", "<sub>", …]` without parsing
/// the rest.
fn sub_of(frame: &str, label: &str) -> Option<String> {
    let rest = frame.trim_start().strip_prefix('[')?.trim_start();
    let rest = rest
        .strip_prefix('"')?
        .strip_prefix(label)?
        .strip_prefix('"')?;
    let rest = rest.trim_start().strip_prefix(',')?.trim_start();
    serde_json::Deserializer::from_str(rest)
        .into_iter::<String>()
        .next()?
        .ok()
}

fn names_user_stream(filter: &Value) -> bool {
    filter
        .get("#h")
        .and_then(Value::as_array)
        .is_some_and(|rooms| rooms.iter().any(|room| room == USER_STREAM_ROOM))
}

impl StreamGate {
    /// A `REQ` or `CLOSE` the app sends.
    pub(crate) fn sent(&mut self, frame: &str, label: &str) {
        let Some(parts) = parts(frame) else { return };
        let Some(Value::String(sub)) = parts.get(1) else {
            return;
        };
        match label {
            "REQ" if parts.iter().skip(2).any(names_user_stream) => {
                // A REQ on the same id starts over (the auth-required retry).
                self.subs
                    .insert(sub.clone(), Verdict::Pending(SnapshotCheck::default()));
            }
            "REQ" => {
                self.subs.remove(sub);
            }
            "CLOSE" => {
                if let Some(verdict) = self.subs.get_mut(sub) {
                    *verdict = Verdict::Skip;
                }
            }
            _ => {}
        }
    }

    /// Whether an `EVENT` frame from the relay may be stored.
    pub(crate) fn keeps_event(&mut self, frame: &str) -> bool {
        if self.subs.is_empty() {
            return true;
        }
        let Some(sub) = sub_of(frame, "EVENT") else {
            return true;
        };
        match self.subs.get_mut(&sub) {
            None => true,
            Some(Verdict::Keep) => true,
            Some(Verdict::Skip) => false,
            Some(Verdict::Pending(check)) => {
                if let Some(event) = parts(frame)
                    .and_then(|mut parts| (parts.len() > 2).then(|| parts.swap_remove(2)))
                {
                    check.feed(&event);
                }
                false
            }
        }
    }

    /// An `EOSE` from the relay: a pending stream decides.
    pub(crate) fn eose(&mut self, frame: &str) {
        let Some(sub) = sub_of(frame, "EOSE") else {
            return;
        };
        if let Some(verdict) = self.subs.get_mut(&sub) {
            if let Verdict::Pending(check) = verdict {
                *verdict = match check.eose() {
                    Some("live" | "gated") => Verdict::Keep,
                    _ => Verdict::Skip,
                };
            }
        }
    }

    /// A `CLOSED` from the relay: later frames on that id are not stored.
    pub(crate) fn closed(&mut self, frame: &str) {
        let Some(sub) = sub_of(frame, "CLOSED") else {
            return;
        };
        if let Some(verdict) = self.subs.get_mut(&sub) {
            *verdict = Verdict::Skip;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Value {
        let text = include_str!("../../../../test-fixtures/user-stream.json");
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn snapshot_checks_follow_the_shared_part1_cases() {
        for case in fixture()["part1"].as_array().unwrap() {
            let mut check = SnapshotCheck::default();
            for event in case["events"].as_array().unwrap() {
                check.feed(event);
            }
            let accepted = check.eose().map(str::to_owned);
            let name = case["name"].as_str().unwrap();
            assert_eq!(
                accepted.is_some(),
                case["expect"]["accept"].as_bool().unwrap(),
                "{name}"
            );
            if let Some(mode) = accepted {
                assert_eq!(mode, case["expect"]["mode"].as_str().unwrap(), "{name}");
            }
        }
    }

    #[test]
    fn seq_and_epoch_limits() {
        let part = |seq: Value, epoch: &str| {
            let mut check = SnapshotCheck::default();
            let content = json!({"v": 1, "op": "snapshot", "part": 1, "parts": 1,
                "mode": "live", "epoch": epoch, "kinds": USER_STREAM_KINDS,
                "rooms": {"a": seq}});
            check.feed(&json!({"kind": 24220, "content": content.to_string()}));
            check.eose().is_some()
        };
        assert!(part(json!(MAX_SEQ), "e"));
        assert!(!part(json!(MAX_SEQ + 1), "e"));
        assert!(part(json!(1), &"e".repeat(64)));
        assert!(!part(json!(1), &"e".repeat(65)));
    }

    fn part1_frame(sub: &str, id: &str, mode: &str, parts: u64, bad_kinds: bool) -> String {
        let kinds: Vec<u64> = if bad_kinds {
            USER_STREAM_KINDS[1..].to_vec()
        } else {
            USER_STREAM_KINDS.to_vec()
        };
        let content = json!({"v": 1, "op": "snapshot", "part": 1, "parts": parts,
            "mode": mode, "epoch": "e1", "kinds": kinds, "rooms": {}});
        json!(["EVENT", sub, {"id": id, "kind": 24220, "content": content.to_string()}]).to_string()
    }

    #[test]
    fn store_skip_follows_the_shared_cases() {
        let sub = "stream-1";
        let req = json!(["REQ", sub, {"#h": ["$joined"], "kinds": USER_STREAM_KINDS, "limit": 0}])
            .to_string();
        for case in fixture()["storeSkip"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let mut gate = StreamGate::default();
            gate.sent(&req, "REQ");
            let mut stored = Vec::new();
            for frame in case["frames"].as_array().unwrap() {
                if frame["eose"] == true {
                    gate.eose(&json!(["EOSE", sub]).to_string());
                } else if frame["req"] == true {
                    gate.sent(&req, "REQ");
                } else if frame["close"] == true {
                    gate.sent(&json!(["CLOSE", sub]).to_string(), "CLOSE");
                } else if let Some(reason) = frame["closed"].as_str() {
                    gate.closed(&json!(["CLOSED", sub, reason]).to_string());
                } else {
                    let id = frame["id"].as_str().unwrap();
                    let kind = frame["kind"].as_u64().unwrap();
                    let text = match frame["mode"].as_str() {
                        Some(mode) => part1_frame(
                            sub,
                            id,
                            mode,
                            frame["parts"].as_u64().unwrap_or(1),
                            frame["badKinds"] == true,
                        ),
                        None if kind == USER_STREAM_KIND => json!(["EVENT", sub,
                            {"id": id, "kind": kind, "content": "{\"rooms\":{}}"}])
                        .to_string(),
                        None => json!(["EVENT", sub, {"id": id, "kind": kind}]).to_string(),
                    };
                    // Kind 24220 is never a stored kind (classify).
                    if gate.keeps_event(&text) && kind != USER_STREAM_KIND {
                        stored.push(id.to_owned());
                    }
                }
            }
            let expected: Vec<String> = case["stored"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| id.as_str().unwrap().to_owned())
                .collect();
            assert_eq!(stored, expected, "{name}");
        }
    }

    #[test]
    fn other_subscriptions_are_untouched() {
        let mut gate = StreamGate::default();
        gate.sent(
            &json!(["REQ", "live-1", {"#h": ["room-a"]}]).to_string(),
            "REQ",
        );
        assert!(gate.keeps_event(&json!(["EVENT", "live-1", {"kind": 9}]).to_string()));
        gate.sent(
            &json!(["REQ", "stream-1", {"#h": ["$joined"]}]).to_string(),
            "REQ",
        );
        assert!(gate.keeps_event(&json!(["EVENT", "live-1", {"kind": 9}]).to_string()));
        assert!(!gate.keeps_event(&json!(["EVENT", "stream-1", {"kind": 9}]).to_string()));
        assert_eq!(
            sub_of(r#"[ "EVENT" , "s\"x" ,{}]"#, "EVENT").as_deref(),
            Some("s\"x")
        );
    }
}
