//! App update manifest — which app version each platform should run.
//!
//! The operator keeps one JSON file (path in `KUZZ_APP_UPDATE_FILE`) and edits
//! it to publish a release, open it to testers first, or raise the minimum
//! supported version. The relay re-reads it on every request, so no redeploy
//! is needed. Without the env var or file the feature is off: the endpoint
//! answers 204 and no connection is refused.
//!
//! ```json
//! {
//!   "enforce_on_connect": false,
//!   "testers": ["<hex pubkey>"],
//!   "platforms": {
//!     "windows-x86_64": {
//!       "min_version": "0.6.1",
//!       "stable": { "version": "0.6.2", "url": "…", "signature": "…",
//!                   "notes": "…", "pub_date": "2026-10-23T00:00:00Z" },
//!       "beta":   { "version": "0.6.3", "url": "…", "signature": "…" }
//!     }
//!   }
//! }
//! ```
//!
//! Routes and gates:
//!
//! - `GET /api/app-update/{platform}?current=<version>` (public). The body is
//!   the Tauri updater "dynamic server" shape (`version`, `url`, `signature`,
//!   `notes`, `pub_date`) plus `min_version`, `update_available` and
//!   `required`, so desktop and mobile read the same answer. Testers
//!   (`X-Kuzz-Pubkey` header) get `beta` when it is newer than `stable`.
//! - WebSocket upgrade: an app that announces itself with
//!   `X-Kuzz-App: <platform>/<version>` below that platform's `min_version` is
//!   refused with `426 Upgrade Required` when `enforce_on_connect` is true.
//!   Clients that send no header (older apps, agents, CLI) are never refused.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::PathBuf;

use axum::{
    extract::{Path, Query},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
};
use serde::{Deserialize, Serialize};

/// Env var naming the manifest file.
pub const MANIFEST_ENV: &str = "KUZZ_APP_UPDATE_FILE";
/// Header an app sends on WebSocket upgrade: `<platform>/<version>`.
pub const APP_HEADER: &str = "x-kuzz-app";
/// Header naming the signed-in account, used only to pick the tester build.
pub const PUBKEY_HEADER: &str = "x-kuzz-pubkey";
/// Reason body for a refused upgrade; clients match on it.
pub const UPDATE_REQUIRED: &str = "update-required";

/// The operator's manifest file.
#[derive(Debug, Default, Deserialize)]
pub struct Manifest {
    /// Refuse WebSocket upgrades from announced apps below `min_version`.
    #[serde(default)]
    pub enforce_on_connect: bool,
    /// Hex pubkeys that see `beta` releases first.
    #[serde(default)]
    pub testers: Vec<String>,
    /// Keyed by platform, e.g. `windows-x86_64`, `darwin-aarch64`, `android`.
    #[serde(default)]
    pub platforms: HashMap<String, PlatformEntry>,
}

/// One platform's releases.
#[derive(Debug, Default, Deserialize)]
pub struct PlatformEntry {
    /// Apps below this version must update before they can be used.
    pub min_version: Option<String>,
    /// Release offered to everyone.
    pub stable: Option<Release>,
    /// Release offered to testers while it is newer than `stable`.
    pub beta: Option<Release>,
}

/// One downloadable build.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Release {
    /// Visible version, `MAJOR.MINOR.PATCH`.
    pub version: String,
    /// Download URL of the installer / update bundle.
    pub url: String,
    /// Tauri updater signature; empty for platforms that verify otherwise
    /// (Android checks the APK signing key).
    #[serde(default)]
    pub signature: String,
    /// Release notes shown in the update prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// RFC 3339 publish time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pub_date: Option<String>,
}

/// Endpoint answer.
#[derive(Debug, Serialize, PartialEq)]
pub struct UpdateAnswer {
    /// The release this caller should run.
    #[serde(flatten)]
    pub release: ReleaseView,
    /// Oldest version still allowed on this platform.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_version: Option<String>,
    /// The caller's `current` is older than `release.version`.
    pub update_available: bool,
    /// The caller's `current` is older than `min_version`: it must update.
    pub required: bool,
}

/// Serializable copy of [`Release`] (kept separate so `UpdateAnswer` can
/// derive `PartialEq` for tests without widening `Release`).
#[derive(Debug, Serialize, PartialEq)]
pub struct ReleaseView {
    /// See [`Release::version`].
    pub version: String,
    /// See [`Release::url`].
    pub url: String,
    /// See [`Release::signature`].
    pub signature: String,
    /// See [`Release::notes`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// See [`Release::pub_date`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pub_date: Option<String>,
}

impl From<&Release> for ReleaseView {
    fn from(r: &Release) -> Self {
        Self {
            version: r.version.clone(),
            url: r.url.clone(),
            signature: r.signature.clone(),
            notes: r.notes.clone(),
            pub_date: r.pub_date.clone(),
        }
    }
}

/// `MAJOR.MINOR.PATCH`, ignoring any `-pre` / `+build` suffix. Missing parts
/// count as 0. Returns `None` for anything that is not numeric.
pub fn parse_version(raw: &str) -> Option<[u64; 3]> {
    let core = raw.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    if core.is_empty() {
        return None;
    }
    let mut out = [0u64; 3];
    for (i, part) in core.split('.').enumerate() {
        if i >= 3 {
            return None;
        }
        out[i] = part.parse().ok()?;
    }
    Some(out)
}

fn cmp_versions(a: &str, b: &str) -> Option<Ordering> {
    Some(parse_version(a)?.cmp(&parse_version(b)?))
}

/// True when `current` is known and strictly older than `other`.
fn is_older(current: Option<&str>, other: &str) -> bool {
    current
        .and_then(|c| cmp_versions(c, other))
        .is_some_and(|o| o == Ordering::Less)
}

impl Manifest {
    fn is_tester(&self, pubkey: Option<&str>) -> bool {
        pubkey.is_some_and(|p| {
            let p = p.trim();
            self.testers.iter().any(|t| t.eq_ignore_ascii_case(p))
        })
    }

    /// What `platform` at `current` should be told. `None` when the platform
    /// has no release.
    pub fn answer(
        &self,
        platform: &str,
        current: Option<&str>,
        pubkey: Option<&str>,
    ) -> Option<UpdateAnswer> {
        let entry = self.platforms.get(platform)?;
        let beta = entry
            .beta
            .as_ref()
            .filter(|_| self.is_tester(pubkey))
            .filter(|beta| match &entry.stable {
                Some(stable) => cmp_versions(&beta.version, &stable.version)
                    .is_some_and(|o| o == Ordering::Greater),
                None => true,
            });
        let release = beta.or(entry.stable.as_ref())?;
        let required = entry
            .min_version
            .as_deref()
            .is_some_and(|min| is_older(current, min));
        Some(UpdateAnswer {
            release: release.into(),
            min_version: entry.min_version.clone(),
            update_available: is_older(current, &release.version),
            required,
        })
    }

    /// Whether an upgrade announcing `app_header` must be refused.
    pub fn refuses_connection(&self, app_header: Option<&str>) -> bool {
        if !self.enforce_on_connect {
            return false;
        }
        let Some((platform, version)) = app_header.and_then(|h| h.trim().split_once('/')) else {
            return false;
        };
        let Some(min) = self
            .platforms
            .get(platform)
            .and_then(|e| e.min_version.as_deref())
        else {
            return false;
        };
        is_older(Some(version), min)
    }
}

fn manifest_path() -> Option<PathBuf> {
    std::env::var_os(MANIFEST_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Read the manifest. Missing env var or file means "feature off"; a file
/// that does not parse is logged and treated the same, so a bad edit never
/// locks anyone out.
pub async fn load() -> Option<Manifest> {
    let path = manifest_path()?;
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "app update manifest unreadable");
            return None;
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(manifest) => Some(manifest),
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "app update manifest invalid");
            None
        }
    }
}

/// Query for [`app_update`].
#[derive(Deserialize)]
pub struct UpdateQuery {
    /// The caller's installed version.
    pub current: Option<String>,
}

/// `GET /api/app-update/{platform}` — public, no authentication.
pub async fn app_update(
    Path(platform): Path<String>,
    Query(query): Query<UpdateQuery>,
    headers: HeaderMap,
) -> Response {
    let pubkey = headers.get(PUBKEY_HEADER).and_then(|v| v.to_str().ok());
    let answer = load()
        .await
        .and_then(|m| m.answer(&platform, query.current.as_deref(), pubkey));
    let mut response = match answer {
        Some(answer) => Json(answer).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

/// Pre-upgrade gate for the relay WebSocket: `Some(response)` refuses.
pub async fn connect_gate(headers: &HeaderMap) -> Option<Response> {
    let app = headers.get(APP_HEADER)?.to_str().ok()?;
    let manifest = load().await?;
    manifest
        .refuses_connection(Some(app))
        .then(|| (StatusCode::UPGRADE_REQUIRED, UPDATE_REQUIRED).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(version: &str) -> Release {
        Release {
            version: version.into(),
            url: format!("https://example.test/{version}"),
            signature: "sig".into(),
            notes: None,
            pub_date: None,
        }
    }

    fn manifest() -> Manifest {
        let mut platforms = HashMap::new();
        platforms.insert(
            "windows-x86_64".to_string(),
            PlatformEntry {
                min_version: Some("0.6.2".into()),
                stable: Some(release("0.6.3")),
                beta: Some(release("0.6.4")),
            },
        );
        Manifest {
            enforce_on_connect: true,
            testers: vec!["AABB".into()],
            platforms,
        }
    }

    #[test]
    fn parses_versions() {
        assert_eq!(parse_version("0.6.1"), Some([0, 6, 1]));
        assert_eq!(parse_version("v1.2"), Some([1, 2, 0]));
        assert_eq!(parse_version("0.6.1-beta+5"), Some([0, 6, 1]));
        assert_eq!(parse_version("0.6.x"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version(""), None);
        assert!(is_older(Some("0.5.26"), "0.6.1"));
        assert!(is_older(Some("0.6.9"), "0.6.10"));
        assert!(!is_older(Some("0.6.10"), "0.6.10"));
        assert!(!is_older(None, "0.6.10"));
    }

    #[test]
    fn everyone_gets_stable_and_testers_get_newer_beta() {
        let m = manifest();
        let public = m.answer("windows-x86_64", Some("0.6.3"), None).unwrap();
        assert_eq!(public.release.version, "0.6.3");
        assert!(!public.update_available);
        assert!(!public.required);

        let tester = m
            .answer("windows-x86_64", Some("0.6.3"), Some("aabb"))
            .unwrap();
        assert_eq!(tester.release.version, "0.6.4");
        assert!(tester.update_available);
    }

    #[test]
    fn beta_older_than_stable_is_ignored() {
        let mut m = manifest();
        m.platforms.get_mut("windows-x86_64").unwrap().beta = Some(release("0.6.3"));
        let tester = m.answer("windows-x86_64", None, Some("aabb")).unwrap();
        assert_eq!(tester.release.url, "https://example.test/0.6.3");
    }

    #[test]
    fn below_min_version_is_required() {
        let m = manifest();
        let old = m.answer("windows-x86_64", Some("0.6.1"), None).unwrap();
        assert!(old.required && old.update_available);
        let unknown = m.answer("windows-x86_64", None, None).unwrap();
        assert!(!unknown.required && !unknown.update_available);
        assert!(m.answer("android", Some("0.6.1"), None).is_none());
    }

    #[test]
    fn connection_gate_only_refuses_announced_old_apps() {
        let mut m = manifest();
        assert!(m.refuses_connection(Some("windows-x86_64/0.6.1")));
        assert!(!m.refuses_connection(Some("windows-x86_64/0.6.2")));
        assert!(!m.refuses_connection(None));
        assert!(!m.refuses_connection(Some("garbage")));
        assert!(!m.refuses_connection(Some("windows-x86_64/not-a-version")));
        assert!(!m.refuses_connection(Some("android/0.1.0")));
        m.enforce_on_connect = false;
        assert!(!m.refuses_connection(Some("windows-x86_64/0.6.1")));
    }

    #[test]
    fn answer_serializes_in_tauri_shape() {
        let m = manifest();
        let json =
            serde_json::to_value(m.answer("windows-x86_64", Some("0.6.1"), None).unwrap()).unwrap();
        assert_eq!(json["version"], "0.6.3");
        assert_eq!(json["url"], "https://example.test/0.6.3");
        assert_eq!(json["signature"], "sig");
        assert_eq!(json["min_version"], "0.6.2");
        assert_eq!(json["required"], true);
        assert!(json.get("notes").is_none());
    }
}
