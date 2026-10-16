//! Shared validation for channel-scoped thread names.

/// Maximum name weight: ASCII characters cost one, other Unicode scalars two.
pub const MAX_THREAD_NAME_WEIGHT: usize = 40;

/// Validate the name event envelope and return its thread head reference.
/// A plain e tag deliberately carries no NIP-10 reply/root marker.
pub fn thread_name_target(event: &nostr::Event) -> Result<nostr::EventId, &'static str> {
    validate_thread_name(&event.content)?;
    let targets: Vec<_> = event
        .tags
        .iter()
        .filter(|t| t.as_slice()[0] == "e")
        .collect();
    let channels: Vec<_> = event
        .tags
        .iter()
        .filter(|t| t.as_slice()[0] == "h")
        .collect();
    if targets.len() != 1
        || channels.len() != 1
        || targets[0].as_slice().len() != 2
        || channels[0].as_slice().len() != 2
    {
        return Err("thread name requires one plain h tag and one plain e tag");
    }
    uuid::Uuid::parse_str(channels[0].content().ok_or("missing channel")?)
        .map_err(|_| "invalid channel UUID")?;
    let target = targets[0].content().ok_or("missing thread head")?;
    if target.len() != 64
        || !target
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err("thread head must be 64 lowercase hex characters");
    }
    nostr::EventId::from_hex(target).map_err(|_| "invalid thread head ID")
}

/// Validate a single-line name. Empty content clears the name.
pub fn validate_thread_name(name: &str) -> Result<(), &'static str> {
    if name.trim() != name
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
    {
        return Err("thread name must be trimmed and contain no control characters");
    }
    if name
        .chars()
        .map(|c| if c.is_ascii() { 1 } else { 2 })
        .sum::<usize>()
        > MAX_THREAD_NAME_WEIGHT
    {
        return Err("thread name exceeds 40 English or 20 Korean characters");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_ambiguous_channels_and_reply_markers() {
        let channel = uuid::Uuid::new_v4().to_string();
        let target = "ab".repeat(32);
        let tags = vec![vec!["h", channel.as_str()], vec!["e", target.as_str()]];
        let build = |tags: Vec<Vec<&str>>| {
            nostr::EventBuilder::new(
                nostr::Kind::Custom(crate::kind::KIND_THREAD_NAME as u16),
                "작업",
            )
            .tags(tags.into_iter().map(|tag| nostr::Tag::parse(tag).unwrap()))
            .sign_with_keys(&nostr::Keys::generate())
            .unwrap()
        };
        assert!(thread_name_target(&build(tags.clone())).is_ok());
        for bad in [
            vec![
                vec!["h", channel.as_str()],
                vec!["e", target.as_str(), "", "reply"],
            ],
            vec![
                vec!["h", channel.as_str()],
                vec!["e", target.as_str(), "", "root"],
            ],
            vec![vec!["h", "invalid"], vec!["e", target.as_str()]],
            vec![vec!["h", channel.as_str()], vec!["e", "missing"]],
            vec![vec!["h", channel.as_str()]],
            [tags.clone(), vec![vec!["h", "different"]]].concat(),
            [tags.clone(), vec![vec!["e", "cd"]]].concat(),
        ] {
            assert!(thread_name_target(&build(bad)).is_err());
        }
    }

    #[test]
    fn validates_names_and_clear_at_the_language_boundaries() {
        for name in [
            "".into(),
            "a".repeat(40),
            "가".repeat(20),
            format!("{}{}", "가".repeat(10), "a".repeat(20)),
        ] {
            assert!(validate_thread_name(&name).is_ok(), "{name:?}");
        }
        for name in [
            "a".repeat(41),
            "가".repeat(21),
            format!("{}a", "가".repeat(20)),
            " leading".into(),
            "trailing ".into(),
            "line\nbreak".into(),
            "tab\there".into(),
        ] {
            assert!(validate_thread_name(&name).is_err(), "{name:?}");
        }
    }
}
