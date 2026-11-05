//! Finding groups and managing how people reach them: public addresses,
//! invite links, and who may share them.
//!
//! The relay decides who may do each of these. Handles and codes are checked
//! here because the relay rebuilds the signed URL from the decoded path, so a
//! value that needs encoding fails as an auth error instead of a clear one.

use crate::client::KuzzClient;
use crate::error::{classify_relay, CliError};
use crate::validate::{percent_encode, validate_uuid};
use crate::GroupsCmd;

#[derive(Debug, PartialEq)]
enum GroupLink {
    Code(String),
    Handle(String),
}

fn is_code(value: &str) -> bool {
    (22..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn is_handle(value: &str) -> bool {
    (3..=32).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn normalize_handle(input: &str) -> Result<String, CliError> {
    let trimmed = input.trim();
    let handle = trimmed.strip_prefix('@').unwrap_or(trimmed).to_lowercase();
    if is_handle(&handle) {
        Ok(handle)
    } else {
        Err(CliError::Usage(format!(
            "invalid handle {input:?}: use 3-32 of a-z, 0-9, and _"
        )))
    }
}

fn link_from_segment(segment: &str) -> Option<GroupLink> {
    if let Some(handle) = segment
        .strip_prefix('@')
        .or_else(|| segment.strip_prefix("%40"))
    {
        let handle = handle.to_lowercase();
        return is_handle(&handle).then_some(GroupLink::Handle(handle));
    }
    is_code(segment).then(|| GroupLink::Code(segment.to_string()))
}

/// Same forms as the app: `https://<host>/g/<code or @handle>`,
/// `kuzz://join/<code or @handle>`, a bare code, or `@handle`.
fn parse_group_link(input: &str) -> Option<GroupLink> {
    let text = input.trim();
    let Some((scheme, rest)) = text.split_once("://") else {
        return link_from_segment(text);
    };
    if rest.contains(['?', '#']) || rest.contains(char::is_whitespace) {
        return None;
    }
    let scheme = scheme.to_ascii_lowercase();
    let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
    match (scheme.as_str(), parts.as_slice()) {
        ("https" | "http", [host, "g", target]) if !host.is_empty() && !host.contains('@') => {
            link_from_segment(target)
        }
        (scheme, [join, target])
            if (scheme == "kuzz" || scheme.contains("-demo-"))
                && join.eq_ignore_ascii_case("join") =>
        {
            link_from_segment(target)
        }
        _ => None,
    }
}

fn search_path(query: &str) -> String {
    format!("/api/groups/search?q={}", percent_encode(query))
}

fn group_path(channel: &str, suffix: &str) -> Result<String, CliError> {
    validate_uuid(channel)?;
    Ok(format!("/api/groups/{channel}{suffix}"))
}

async fn print_get(client: &KuzzClient, path: &str) -> Result<(), CliError> {
    println!("{}", client.get_authed(path).await?);
    Ok(())
}

pub async fn dispatch(cmd: GroupsCmd, client: &KuzzClient) -> Result<(), CliError> {
    run(cmd, client).await.map_err(classify_relay)
}

async fn run(cmd: GroupsCmd, client: &KuzzClient) -> Result<(), CliError> {
    match cmd {
        GroupsCmd::Search { query } => print_get(client, &search_path(&query)).await,
        GroupsCmd::Get { channel, handle } => {
            let path = match (channel, handle) {
                (Some(channel), _) => group_path(&channel, "")?,
                (None, Some(handle)) => {
                    format!("/api/groups/by-handle/{}", normalize_handle(&handle)?)
                }
                (None, None) => unreachable!("clap requires --channel or --handle"),
            };
            print_get(client, &path).await
        }
        GroupsCmd::Handle {
            channel,
            set,
            clear: _,
        } => {
            let path = group_path(&channel, "/handle")?;
            let handle = set.as_deref().map(normalize_handle).transpose()?;
            let body = serde_json::json!({ "handle": handle });
            println!("{}", client.put_json_once_authed(&path, &body).await?);
            Ok(())
        }
        GroupsCmd::InviteLink { channel, reset } => {
            if reset {
                let path = group_path(&channel, "/invite-link/reset")?;
                let body = serde_json::json!({});
                println!("{}", client.post_json_once_authed(&path, &body).await?);
                Ok(())
            } else {
                print_get(client, &group_path(&channel, "/invite-link")?).await
            }
        }
        GroupsCmd::InvitePolicy { channel, set } => {
            let path = group_path(&channel, "/invite-policy")?;
            match set {
                Some(policy) => {
                    let body = serde_json::json!({ "policy": policy.as_str() });
                    println!("{}", client.put_json_once_authed(&path, &body).await?);
                    Ok(())
                }
                None => print_get(client, &path).await,
            }
        }
        GroupsCmd::Preview { link } => {
            let path = match parse_group_link(&link) {
                Some(GroupLink::Code(code)) => format!("/api/groups/invite/{code}"),
                Some(GroupLink::Handle(handle)) => format!("/api/groups/by-handle/{handle}"),
                None => {
                    return Err(CliError::Usage(format!(
                        "not a group link, invite code, or @handle: {link:?}"
                    )))
                }
            };
            print_get(client, &path).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = "AbCdEfGhIjKlMnOpQrStUv_-";

    #[test]
    fn handles_are_normalized_like_the_app() {
        assert_eq!(normalize_handle(" @Team_1 ").unwrap(), "team_1");
    }

    #[test]
    fn handles_that_would_need_encoding_are_refused() {
        for bad in ["ab", "a/b", "한글이름", "../auth", &"a".repeat(33)] {
            assert!(normalize_handle(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn links_parse_to_a_code_or_handle() {
        let code = || Some(GroupLink::Code(CODE.to_string()));
        let handle = || Some(GroupLink::Handle("team_1".to_string()));
        assert_eq!(parse_group_link(CODE), code());
        assert_eq!(
            parse_group_link(&format!("https://gateway.kuzz.app/g/{CODE}/")),
            code()
        );
        assert_eq!(parse_group_link(&format!("kuzz://join/{CODE}")), code());
        assert_eq!(parse_group_link("@Team_1"), handle());
        assert_eq!(
            parse_group_link("https://gateway.kuzz.app/g/@team_1"),
            handle()
        );
        assert_eq!(
            parse_group_link("https://gateway.kuzz.app/g/%40team_1"),
            handle()
        );
        assert_eq!(parse_group_link("kuzz://join/@team_1"), handle());
    }

    #[test]
    fn links_elsewhere_are_refused() {
        for bad in [
            "short",
            "https://gateway.kuzz.app?x/g/@team_1",
            "https://gateway.kuzz.app/x/@team_1",
            "https://user@evil.example/g/@team_1",
            "ftp://gateway.kuzz.app/g/@team_1",
            "kuzz://other/@team_1",
        ] {
            assert_eq!(parse_group_link(bad), None, "{bad}");
        }
    }

    #[test]
    fn search_query_is_encoded() {
        assert_eq!(
            search_path("개발 팀&x=1"),
            "/api/groups/search?q=%EA%B0%9C%EB%B0%9C%20%ED%8C%80%26x%3D1"
        );
    }

    #[test]
    fn group_path_refuses_anything_but_a_uuid() {
        assert!(group_path("../../auth/me", "/handle").is_err());
    }
}
