//! Quote preview: a message the reader can see that quotes one from before
//! their join shows the original only inside the quote box, as a short
//! server-cut excerpt (group chat plan §9). The original itself stays hidden.

use axum::{http::StatusCode, Json};
use kuzz_core::{kind::KIND_QUOTE_PREVIEW, TenantContext};
use nostr::{EventBuilder, Filter, Kind, Tag, Timestamp};
use serde_json::Value;
use uuid::Uuid;

use super::{api_error, extract_channel_from_filter, internal_error};
use crate::state::AppState;

type QueryError = (StatusCode, Json<Value>);

/// Bridge filter flag: `{"quote_preview": true, "#h": [channel],
/// "ids": [quoted], "#e": [quoting message]}`.
pub(super) const FLAG: &str = "quote_preview";
/// Most quote previews one `/query` request may ask for.
pub(super) const MAX_PER_REQUEST: usize = 50;
/// Longest excerpt in characters (an ellipsis follows a cut).
const EXCERPT_CHARS: usize = 140;

fn bad_request(message: &str) -> QueryError {
    api_error(StatusCode::BAD_REQUEST, message)
}

fn request(filter: &Filter) -> Result<(Uuid, Vec<u8>, Vec<u8>), QueryError> {
    let channel = extract_channel_from_filter(filter)
        .ok_or_else(|| bad_request("quote_preview requires exactly one #h channel"))?;
    let quoted = match filter.ids.as_ref() {
        Some(ids) if ids.len() == 1 => ids.iter().next().map(|id| id.as_bytes().to_vec()),
        _ => None,
    }
    .ok_or_else(|| bad_request("quote_preview requires exactly one quoted id"))?;
    let quoting = filter
        .generic_tags
        .iter()
        .find(|(tag, _)| tag.as_char() == 'e')
        .and_then(|(_, values)| (values.len() == 1).then(|| values.iter().next()).flatten())
        .and_then(|id| hex::decode(id).ok())
        .filter(|id| id.len() == 32)
        .ok_or_else(|| bad_request("quote_preview requires exactly one #e quoting id"))?;
    Ok((channel, quoted, quoting))
}

/// Plain-text excerpt: the first [`EXCERPT_CHARS`] characters on one line;
/// a message with only attachments reads "Photo" or "File".
fn excerpt(content: &str, tags: &Value) -> String {
    let text = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        let imeta: Vec<&Value> = tags
            .as_array()
            .into_iter()
            .flatten()
            .filter(|tag| tag.get(0).and_then(Value::as_str) == Some("imeta"))
            .collect();
        if imeta.is_empty() {
            return String::new();
        }
        let image = imeta.iter().any(|tag| {
            tag.as_array()
                .into_iter()
                .flatten()
                .any(|part| part.as_str().is_some_and(|p| p.starts_with("m image/")))
        });
        return if image { "Photo" } else { "File" }.to_string();
    }
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(EXCERPT_CHARS).collect();
    if chars.next().is_some() {
        format!("{}…", head.trim_end())
    } else {
        head
    }
}

/// A relay-signed [`KIND_QUOTE_PREVIEW`] for one quote, or nothing when the
/// reader may not see it (no access, not a quote, not earlier, deleted).
pub(super) async fn query(
    state: &AppState,
    tenant: &TenantContext,
    filter: &Filter,
    accessible_channels: &[Uuid],
    reader: &[u8],
) -> Result<Vec<Value>, QueryError> {
    let (channel, quoted, quoting) = request(filter)?;
    if !accessible_channels.contains(&channel) {
        return Ok(Vec::new());
    }
    let Some(preview) = state
        .db
        .quote_preview(tenant.community(), channel, reader, &quoting, &quoted)
        .await
        .map_err(|e| internal_error(&format!("quote preview: {e}")))?
    else {
        return Ok(Vec::new());
    };
    let quoted_hex = hex::encode(&quoted);
    let author_hex = hex::encode(&preview.author);
    let channel_hex = channel.to_string();
    let tags = [
        ["e", quoted_hex.as_str()],
        ["d", quoted_hex.as_str()],
        ["h", channel_hex.as_str()],
        ["p", author_hex.as_str()],
    ]
    .into_iter()
    .map(Tag::parse)
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| internal_error(&format!("quote preview tags: {e}")))?;
    let content = serde_json::json!({
        "excerpt": excerpt(&preview.content, &preview.tags),
        "kind": preview.kind,
        "created_at": preview.created_at.timestamp(),
    });
    let event = EventBuilder::new(Kind::Custom(KIND_QUOTE_PREVIEW as u16), content.to_string())
        .tags(tags)
        .custom_created_at(Timestamp::now())
        .sign_with_keys(&state.relay_keypair)
        .map_err(|e| internal_error(&format!("quote preview signing: {e}")))?;
    Ok(vec![serde_json::to_value(event).map_err(|e| {
        internal_error(&format!("quote preview serialization: {e}"))
    })?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excerpt_cuts_long_text_and_names_attachments() {
        let long = "word ".repeat(100);
        let cut = excerpt(&long, &Value::Null);
        assert!(cut.ends_with('…'));
        assert!(cut.chars().count() <= EXCERPT_CHARS + 1);
        assert_eq!(excerpt("  hi\n there ", &Value::Null), "hi there");
        let photo = serde_json::json!([["imeta", "url https://x/media/a.jpg", "m image/jpeg"]]);
        assert_eq!(excerpt("", &photo), "Photo");
        let file = serde_json::json!([["imeta", "url https://x/media/a.pdf", "m application/pdf"]]);
        assert_eq!(excerpt("", &file), "File");
        assert_eq!(excerpt("", &Value::Null), "");
    }

    #[test]
    fn request_needs_one_channel_quoted_and_quoting_id() {
        let ok: Filter = serde_json::from_value(serde_json::json!({
            "#h": ["11111111-1111-4111-8111-111111111111"],
            "ids": [format!("{:064x}", 1)],
            "#e": [format!("{:064x}", 2)],
        }))
        .expect("filter");
        assert!(request(&ok).is_ok());
        let no_quoting: Filter = serde_json::from_value(serde_json::json!({
            "#h": ["11111111-1111-4111-8111-111111111111"],
            "ids": [format!("{:064x}", 1)],
        }))
        .expect("filter");
        assert!(request(&no_quoting).is_err());
    }
}
