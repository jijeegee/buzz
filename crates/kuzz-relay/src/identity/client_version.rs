//! Minimum app version for sign-in and session refresh.
//!
//! The minimum is the app update manifest's per-platform `min_version`
//! (`KUZZ_APP_UPDATE_FILE`, see [`crate::api::app_update`]), the same value
//! the WebSocket upgrade check reads. Apps name themselves with
//! `X-Kuzz-Client: <platform>/<version>` on every `/auth/*` call, using the
//! manifest's platform names (`windows-x86_64`, `darwin-aarch64`,
//! `android`, ...) plus `ios`, `cli` and `acp`. The browser leg of a sign-in,
//! which cannot set headers, carries the same value in the `kuzz_client`
//! query parameter. The rule is deterministic:
//!
//! 1. A request that names a known platform is judged by that platform's
//!    `min_version` only: refused when its version is below it, allowed
//!    otherwise (including when the manifest has no minimum for it). So the
//!    CLI and `kuzz-acp` are gated only when the manifest lists `cli` /
//!    `acp` with a minimum.
//! 2. A request with no usable value (missing, malformed, unknown platform)
//!    is judged by the session's client kind: a `desktop` session is refused
//!    when any desktop platform has a minimum, a `mobile` session when
//!    `android` or `ios` has one (the refusal names the lowest such
//!    minimum). CLI, web and agent sessions are never refused this way.
//!
//! No manifest, or one that cannot be read or parsed, means no gate (logged
//! by [`crate::api::app_update::load`]). A `min_version` that is not a
//! version is logged as an error and ignored, so a bad edit never locks
//! anyone out.

use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;

use crate::api::app_update::{self, parse_version, Manifest};

/// Header an app sends on `/auth/*` calls: `<platform>/<version>`.
pub const CLIENT_HEADER: &str = "x-kuzz-client";
/// Query parameter carrying the same value on the browser sign-in start.
pub const CLIENT_QUERY: &str = "kuzz_client";
/// Error code of a refusal.
pub const UPDATE_REQUIRED: &str = "update_required";

/// What kind of app a manifest platform name belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Desktop,
    Mobile,
    Tool,
}

/// `windows-*`, `darwin-*`, `linux-*` are desktop; `android`, `ios` mobile;
/// `cli`, `acp` tools. Anything else is not a known platform.
fn kind_of(platform: &str) -> Option<Kind> {
    match platform {
        "android" | "ios" => Some(Kind::Mobile),
        "cli" | "acp" => Some(Kind::Tool),
        _ => {
            let (os, arch) = platform.split_once('-')?;
            (matches!(os, "windows" | "darwin" | "linux")
                && !arch.is_empty()
                && arch.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
            .then_some(Kind::Desktop)
        }
    }
}

/// `<platform>/<version>` with a known platform and a parseable version.
fn parse_declared(raw: &str) -> Option<(&str, [u64; 3])> {
    let (platform, version) = raw.trim().split_once('/')?;
    kind_of(platform)?;
    Some((platform, parse_version(version)?))
}

/// Whether `declared` names a known platform with a parseable version, so the
/// session kind is not needed to judge it (rule 1).
pub fn names_app(declared: Option<&str>) -> bool {
    declared.and_then(parse_declared).is_some()
}

/// `platform`'s usable minimum. One that is not a version is logged and
/// ignored.
fn minimum<'a>(manifest: &'a Manifest, platform: &str) -> Option<(&'a str, [u64; 3])> {
    let raw = manifest.platforms.get(platform)?.min_version.as_deref()?;
    match parse_version(raw) {
        Some(parsed) => Some((raw, parsed)),
        None => {
            tracing::error!(
                platform,
                min_version = raw,
                "app update manifest min_version is not a version; not enforced"
            );
            None
        }
    }
}

/// Lowest usable minimum among `client`'s platforms (rule 2).
fn unnamed_minimum<'a>(manifest: &'a Manifest, client: &str) -> Option<&'a str> {
    let kind = match client {
        "desktop" => Kind::Desktop,
        "mobile" => Kind::Mobile,
        _ => return None,
    };
    manifest
        .platforms
        .keys()
        .filter(|platform| kind_of(platform) == Some(kind))
        .filter_map(|platform| minimum(manifest, platform))
        .min_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(b.0)))
        .map(|(raw, _)| raw)
}

/// Whether a request without a usable platform name could be refused for a
/// session of `client` kind (rule 2). Lets callers skip a lookup.
pub fn gates_unnamed(manifest: &Manifest, client: &str) -> bool {
    unnamed_minimum(manifest, client).is_some()
}

/// Apply the rule. `declared`: the header (or query) value; `client`: the
/// session's client kind (`desktop`, `mobile`, `cli`, `web`), when known.
/// `Err(minimum)` refuses.
pub fn check(
    manifest: &Manifest,
    declared: Option<&str>,
    client: Option<&str>,
) -> Result<(), String> {
    if let Some((platform, version)) = declared.and_then(parse_declared) {
        return match minimum(manifest, platform) {
            Some((raw, min)) if version < min => Err(raw.to_owned()),
            _ => Ok(()),
        };
    }
    match client.and_then(|client| unnamed_minimum(manifest, client)) {
        Some(min) => Err(min.to_owned()),
        None => Ok(()),
    }
}

/// [`check`] against the current manifest; no manifest means no gate.
pub async fn check_current(declared: Option<&str>, client: Option<&str>) -> Result<(), String> {
    match app_update::load().await {
        Some(manifest) => check(&manifest, declared, client),
        None => Ok(()),
    }
}

/// The request's [`CLIENT_HEADER`] value, if it is valid UTF-8.
pub fn declared(headers: &HeaderMap) -> Option<&str> {
    headers.get(CLIENT_HEADER)?.to_str().ok()
}

/// `426 {"error", "code": "update_required", "min_version"}`.
pub fn update_required_response(min_version: &str) -> Response {
    (
        StatusCode::UPGRADE_REQUIRED,
        axum::Json(json!({
            "error": "This version of Kuzz is too old. Update Kuzz to continue.",
            "code": UPDATE_REQUIRED,
            "min_version": min_version,
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::app_update::PlatformEntry;

    fn manifest(mins: &[(&str, &str)]) -> Manifest {
        let mut m = Manifest::default();
        for (platform, min) in mins {
            m.platforms.insert(
                (*platform).to_owned(),
                PlatformEntry {
                    min_version: Some((*min).to_owned()),
                    ..PlatformEntry::default()
                },
            );
        }
        m
    }

    #[test]
    fn named_platform_is_judged_by_its_own_minimum() {
        let m = manifest(&[("windows-x86_64", "0.6.2"), ("android", "0.6.2")]);
        assert_eq!(
            check(&m, Some("windows-x86_64/0.6.1"), Some("desktop")),
            Err("0.6.2".into())
        );
        assert_eq!(
            check(&m, Some("windows-x86_64/0.5.26"), None),
            Err("0.6.2".into())
        );
        assert_eq!(
            check(&m, Some("windows-x86_64/0.6.2"), Some("desktop")),
            Ok(())
        );
        assert_eq!(
            check(&m, Some("windows-x86_64/0.6.10"), Some("desktop")),
            Ok(())
        );
        assert_eq!(
            check(&m, Some("android/0.6.1+2026102301"), Some("mobile")),
            Err("0.6.2".into())
        );
        assert_eq!(check(&m, Some("android/0.6.2"), Some("mobile")), Ok(()));
        // A known platform with no minimum: allowed, whatever the session.
        assert_eq!(
            check(&m, Some("darwin-aarch64/0.0.1"), Some("desktop")),
            Ok(())
        );
        assert_eq!(check(&m, Some("ios/0.0.1"), Some("mobile")), Ok(()));
        assert_eq!(check(&m, Some("cli/0.1.0"), Some("desktop")), Ok(()));
    }

    #[test]
    fn missing_or_unusable_header_falls_back_to_session_kind() {
        let m = manifest(&[
            ("windows-x86_64", "0.6.3"),
            ("darwin-aarch64", "0.6.2"),
            ("ios", "0.7.0"),
        ]);
        for declared in [
            None,
            Some(""),
            Some("windows-x86_64"),
            Some("desktop/0.9.0"),
            Some("web/1.0.0"),
            Some("windows-x86_64/x"),
            Some("solaris-sparc/9.0.0"),
        ] {
            // The lowest desktop minimum.
            assert_eq!(
                check(&m, declared, Some("desktop")),
                Err("0.6.2".into()),
                "{declared:?}"
            );
            assert_eq!(
                check(&m, declared, Some("mobile")),
                Err("0.7.0".into()),
                "{declared:?}"
            );
            assert_eq!(check(&m, declared, Some("cli")), Ok(()), "{declared:?}");
            assert_eq!(check(&m, declared, Some("web")), Ok(()), "{declared:?}");
            assert_eq!(check(&m, declared, None), Ok(()), "{declared:?}");
        }
        assert!(gates_unnamed(&m, "desktop") && gates_unnamed(&m, "mobile"));
        assert!(!gates_unnamed(&m, "cli") && !gates_unnamed(&m, "web"));
    }

    #[test]
    fn no_minimum_means_no_gate() {
        let m = Manifest::default();
        assert_eq!(check(&m, None, Some("desktop")), Ok(()));
        assert_eq!(
            check(&m, Some("windows-x86_64/0.0.1"), Some("desktop")),
            Ok(())
        );
        let android_only = manifest(&[("android", "0.6.2")]);
        assert_eq!(check(&android_only, None, Some("desktop")), Ok(()));
        assert_eq!(
            check(&android_only, None, Some("mobile")),
            Err("0.6.2".into())
        );
    }

    #[test]
    fn malformed_minimum_fails_open() {
        let m = manifest(&[("windows-x86_64", "soon"), ("android", "")]);
        assert_eq!(
            check(&m, Some("windows-x86_64/0.0.1"), Some("desktop")),
            Ok(())
        );
        assert_eq!(check(&m, None, Some("desktop")), Ok(()));
        assert_eq!(check(&m, Some("android/0.0.1"), Some("mobile")), Ok(()));
        assert_eq!(check(&m, None, Some("mobile")), Ok(()));
        assert!(!gates_unnamed(&m, "desktop") && !gates_unnamed(&m, "mobile"));
        // A bad entry does not hide a good one of the same kind.
        let mixed = manifest(&[("windows-x86_64", "soon"), ("linux-x86_64", "0.6.2")]);
        assert_eq!(check(&mixed, None, Some("desktop")), Err("0.6.2".into()));
    }

    #[test]
    fn cli_and_acp_are_gated_only_when_listed() {
        let m = manifest(&[("windows-x86_64", "0.6.2")]);
        assert_eq!(check(&m, Some("cli/0.0.1"), Some("cli")), Ok(()));
        assert_eq!(check(&m, Some("acp/0.0.1"), None), Ok(()));
        let listed = manifest(&[("cli", "0.2.0"), ("acp", "0.2.0")]);
        assert_eq!(
            check(&listed, Some("cli/0.1.0"), Some("cli")),
            Err("0.2.0".into())
        );
        assert_eq!(check(&listed, Some("acp/0.1.0"), None), Err("0.2.0".into()));
        // A listed CLI still never gates a CLI session that names nothing.
        assert_eq!(check(&listed, None, Some("cli")), Ok(()));
    }

    #[test]
    fn platform_names() {
        for desktop in ["windows-x86_64", "darwin-aarch64", "linux-x86_64"] {
            assert_eq!(kind_of(desktop), Some(Kind::Desktop), "{desktop}");
        }
        for unknown in [
            "desktop",
            "web",
            "windows-",
            "Windows-x86_64",
            "linux-x/y",
            "",
        ] {
            assert_eq!(kind_of(unknown), None, "{unknown}");
        }
        assert_eq!(kind_of("android"), Some(Kind::Mobile));
        assert_eq!(kind_of("acp"), Some(Kind::Tool));
    }
}
