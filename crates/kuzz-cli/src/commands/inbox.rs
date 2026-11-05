//! What is waiting for this identity: its chats, what it has read, and the
//! threads it follows. Every call reads or changes the signer's own state on
//! the relay, the same state the app shows in Chats.

use serde_json::{json, Value};

use crate::client::KuzzClient;
use crate::error::{classify_relay, CliError};
use crate::validate::{validate_hex64, validate_uuid};

const PAGE_SIZE: u32 = 200;
/// 10,000 rows. A walk that reaches this says so in its output.
const MAX_PAGES: usize = 50;
/// Relay `MAX_MARK_READ_ITEMS`.
const MARK_BATCH: usize = 100;
const PREVIEW_CHARS: usize = 200;
/// Past every message; the relay clamps it to the newest one. A timestamp
/// would miss a message signed by a clock ahead of this one.
const END_OF_CHAT: i64 = i64::MAX;

async fn chat_query(client: &KuzzClient, filter: &Value) -> Result<Value, CliError> {
    let raw = client.query(filter).await.map_err(classify_relay)?;
    serde_json::from_str(&raw)
        .map_err(|e| CliError::Other(format!("relay response is not JSON: {e} ({raw})")))
}

fn list_filter(limit: Option<u32>, before: &Value) -> Value {
    let mut filter = json!({ "chat_list": 1 });
    if let Some(limit) = limit {
        filter["limit"] = json!(limit);
    }
    if !before.is_null() {
        filter["before"] = before.clone();
    }
    filter
}

fn is_unread(row: &Value) -> bool {
    row["unread"].as_i64().unwrap_or(0) > 0 || row["unread_mark"].as_bool().unwrap_or(false)
}

/// Full events carry tags and a signature in every row. Keep what tells an
/// agent whether to open the chat.
fn trim_row(row: &mut Value) {
    let Some(object) = row.as_object_mut() else {
        return;
    };
    object.remove("root");
    let latest = object.get("latest").filter(|latest| latest.is_object());
    let Some(latest) = latest else {
        return;
    };
    let content = latest["content"].as_str().unwrap_or("");
    let mut preview: String = content.chars().take(PREVIEW_CHARS).collect();
    if preview.len() < content.len() {
        preview.push('…');
    }
    let trimmed = json!({
        "id": latest["id"],
        "pubkey": latest["pubkey"],
        "created_at": latest["created_at"],
        "content": preview,
    });
    object.insert("latest".into(), trimmed);
}

fn trim_rows(rows: &mut [Value], full: bool) {
    if !full {
        rows.iter_mut().for_each(trim_row);
    }
}

/// Rows are filtered after the page is cut, so a short or empty page says
/// nothing; only `next` ends the walk.
async fn unread_rows(client: &KuzzClient) -> Result<(Vec<Value>, bool), CliError> {
    let mut rows = Vec::new();
    let mut before = Value::Null;
    for _ in 0..MAX_PAGES {
        let page = chat_query(client, &list_filter(Some(PAGE_SIZE), &before)).await?;
        let page_rows = page["rows"].as_array().into_iter().flatten();
        rows.extend(page_rows.filter(|row| is_unread(row)).cloned());
        before = page["next"].clone();
        if before.is_null() {
            return Ok((rows, false));
        }
    }
    Ok((rows, true))
}

pub async fn cmd_inbox(
    client: &KuzzClient,
    limit: Option<u32>,
    unread: bool,
    before: Option<(i64, String)>,
    full: bool,
) -> Result<(), CliError> {
    let output = if unread {
        let (mut rows, truncated) = unread_rows(client).await?;
        trim_rows(&mut rows, full);
        json!({ "rows": rows, "truncated": truncated })
    } else {
        let before = before.map_or(
            Value::Null,
            |(at_us, key)| json!({ "at_us": at_us, "key": key }),
        );
        let mut page = chat_query(client, &list_filter(limit, &before)).await?;
        if let Some(rows) = page["rows"].as_array_mut() {
            trim_rows(rows, full);
        }
        page
    };
    println!("{output}");
    Ok(())
}

fn target_item(channel: Option<&str>, thread: Option<&str>) -> Result<Value, CliError> {
    match (channel, thread) {
        (Some(channel), _) => {
            validate_uuid(channel)?;
            Ok(json!({ "channel": channel }))
        }
        (None, Some(thread)) => {
            validate_hex64(thread)?;
            Ok(json!({ "thread": thread }))
        }
        (None, None) => Err(CliError::Usage("--channel or --thread is required".into())),
    }
}

fn require_stored(reply: &Value) -> Result<(), CliError> {
    let items = reply["items"].as_array().into_iter().flatten();
    if items.into_iter().any(|item| item["stored"] == json!(false)) {
        return Err(CliError::Usage(
            "nothing changed: you do not follow this thread (see `kuzz threads follow`)".into(),
        ));
    }
    Ok(())
}

pub async fn cmd_mark_read(
    client: &KuzzClient,
    channel: Option<&str>,
    thread: Option<&str>,
    event: Option<&str>,
) -> Result<(), CliError> {
    let mut item = target_item(channel, thread)?;
    match event {
        Some(event) => {
            validate_hex64(event)?;
            item["event"] = json!(event);
        }
        None => item["seq"] = json!(END_OF_CHAT),
    }
    let reply = chat_query(client, &json!({ "mark_read": 1, "items": [item] })).await?;
    require_stored(&reply)?;
    println!("{reply}");
    Ok(())
}

pub async fn cmd_mark_unread(
    client: &KuzzClient,
    channel: Option<&str>,
    thread: Option<&str>,
) -> Result<(), CliError> {
    let item = target_item(channel, thread)?;
    let filter = json!({ "mark_read": 1, "items": [item], "unread_mark": true });
    let reply = chat_query(client, &filter).await?;
    require_stored(&reply)?;
    println!("{reply}");
    Ok(())
}

/// A chat whose only unread state is the mark has no `last_seq`; naming it
/// without a position clears the mark.
fn row_mark_item(row: &Value) -> Option<Value> {
    let mut item = if row["type"] == "thread" {
        json!({ "thread": row["root_id"].as_str()? })
    } else {
        json!({ "channel": row["channel_id"].as_str()? })
    };
    if let Some(seq) = row["last_seq"].as_i64() {
        item["seq"] = json!(seq);
    }
    Some(item)
}

async fn send_marks(client: &KuzzClient, items: &[Value]) -> Result<usize, CliError> {
    let reply = chat_query(client, &json!({ "mark_read": 1, "items": items })).await?;
    let stored = reply["items"].as_array().into_iter().flatten();
    Ok(stored.filter(|item| item["stored"] == json!(true)).count())
}

/// One refused item rolls back its whole batch: a room left mid-walk, or a
/// followed root that was deleted.
fn is_refused(error: &CliError) -> bool {
    matches!(
        error,
        CliError::NotFound(_) | CliError::Relay { status: 403, .. }
    )
}

pub async fn cmd_mark_all_read(client: &KuzzClient) -> Result<(), CliError> {
    let (rows, truncated) = unread_rows(client).await?;
    let items: Vec<Value> = rows.iter().filter_map(row_mark_item).collect();
    let (mut marked, mut skipped) = (0, 0);
    for batch in items.chunks(MARK_BATCH) {
        match send_marks(client, batch).await {
            Ok(count) => marked += count,
            Err(error) if is_refused(&error) => {
                for item in batch {
                    match send_marks(client, std::slice::from_ref(item)).await {
                        Ok(count) => marked += count,
                        Err(error) if is_refused(&error) => skipped += 1,
                        Err(error) => return Err(error),
                    }
                }
            }
            Err(error) => return Err(error),
        }
    }
    println!(
        "{}",
        json!({ "marked": marked, "skipped": skipped, "truncated": truncated })
    );
    Ok(())
}

pub async fn cmd_thread_follow(
    client: &KuzzClient,
    root: &str,
    state: &str,
) -> Result<(), CliError> {
    validate_hex64(root)?;
    let filter = json!({ "thread_follow": 1, "root": root, "state": state });
    println!("{}", chat_query(client, &filter).await?);
    Ok(())
}

pub async fn cmd_thread_search(
    client: &KuzzClient,
    query: &str,
    limit: Option<u32>,
) -> Result<(), CliError> {
    let mut filter = json!({ "thread_search": 1, "q": query });
    if let Some(limit) = limit {
        filter["limit"] = json!(limit);
    }
    println!("{}", chat_query(client, &filter).await?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "abababababababababababababababababababababababababababababababab";

    #[test]
    fn unread_counts_a_mark_or_messages() {
        assert!(is_unread(&json!({ "unread": 2, "unread_mark": false })));
        assert!(is_unread(&json!({ "unread": 0, "unread_mark": true })));
        assert!(!is_unread(&json!({ "unread": 0, "unread_mark": false })));
    }

    #[test]
    fn rows_keep_a_preview_of_the_latest_message() {
        let mut row = json!({
            "key": "k",
            "root": { "id": "r" },
            "latest": {
                "id": "e", "pubkey": "p", "created_at": 5, "sig": "s",
                "tags": [["h", "x"]], "content": "가".repeat(PREVIEW_CHARS + 1),
            },
        });
        trim_row(&mut row);
        assert_eq!(row["key"], "k");
        assert!(row.get("root").is_none());
        assert!(row["latest"].get("sig").is_none());
        assert!(row["latest"].get("tags").is_none());
        let preview = row["latest"]["content"].as_str().unwrap();
        assert_eq!(preview.chars().count(), PREVIEW_CHARS + 1);
        assert!(preview.ends_with('…'));
    }

    #[test]
    fn a_hidden_latest_message_stays_null() {
        let mut row = json!({ "latest": null });
        trim_row(&mut row);
        assert!(row["latest"].is_null());
    }

    #[test]
    fn mark_items_name_the_row_and_its_position() {
        let room = json!({ "type": "room", "channel_id": "c", "last_seq": 7 });
        assert_eq!(
            row_mark_item(&room),
            Some(json!({ "channel": "c", "seq": 7 }))
        );
        let thread = json!({ "type": "thread", "root_id": ROOT, "last_seq": 3 });
        assert_eq!(
            row_mark_item(&thread),
            Some(json!({ "thread": ROOT, "seq": 3 }))
        );
    }

    #[test]
    fn a_mark_only_row_sends_no_position() {
        let row = json!({ "type": "room", "channel_id": "c", "last_seq": null });
        assert_eq!(row_mark_item(&row), Some(json!({ "channel": "c" })));
    }

    #[test]
    fn a_thread_nobody_follows_is_an_error_not_a_quiet_success() {
        assert!(require_stored(&json!({ "items": [{ "stored": false }] })).is_err());
        assert!(require_stored(&json!({ "items": [{ "stored": true }] })).is_ok());
    }

    #[test]
    fn targets_are_checked_before_sending() {
        assert!(target_item(Some("../auth"), None).is_err());
        assert!(target_item(None, Some("short")).is_err());
        assert_eq!(
            target_item(None, Some(ROOT)).unwrap(),
            json!({ "thread": ROOT })
        );
    }

    #[test]
    fn list_filter_sends_only_what_was_asked() {
        assert_eq!(list_filter(None, &Value::Null), json!({ "chat_list": 1 }));
        let before = json!({ "at_us": 1, "key": "k" });
        assert_eq!(
            list_filter(Some(5), &before),
            json!({ "chat_list": 1, "limit": 5, "before": before })
        );
    }
}
