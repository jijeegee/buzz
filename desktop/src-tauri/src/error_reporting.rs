//! Crash and error reporting to Sentry.
//!
//! Off unless the build sets `KUZZ_SENTRY_DSN_DESKTOP`. Reports carry the
//! error, where it happened, app version and OS only: no user, IP, host name,
//! breadcrumbs or request data, and every string is scrubbed for keys,
//! tokens, emails and quoted text (Rust panics quote the string they failed
//! on, which can be message text). The same error is sent once per run.

use std::collections::{hash_map::DefaultHasher, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{LazyLock, Mutex};

use regex::Regex;
use sentry::protocol::Event;

/// Most reports one run may send; a crash loop must not use up the quota.
const MAX_EVENTS_PER_RUN: usize = 20;
/// Longest error text kept, after scrubbing.
const MAX_TEXT_CHARS: usize = 300;

const DSN: Option<&str> = option_env!("KUZZ_DESKTOP_SENTRY_DSN");

/// Starts reporting when this build has a DSN. Keep the guard for the whole
/// process: dropping it stops reporting.
pub fn init() -> Option<sentry::ClientInitGuard> {
    let dsn = DSN.map(str::trim).filter(|dsn| !dsn.is_empty())?;
    let seen = Mutex::new(SeenErrors::default());
    let mut options = sentry::ClientOptions::new()
        .release(env!("CARGO_PKG_VERSION"))
        .environment(env!("KUZZ_DESKTOP_BUILD_ENV"))
        .send_default_pii(false)
        .max_breadcrumbs(0)
        .attach_stacktrace(true)
        .before_send(move |event| {
            let event = scrub_event(event);
            let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
            seen.admit(&event).then_some(event)
        });
    // The default is the computer's name, which can be a person's name.
    options.server_name = None;
    let guard = sentry::init((dsn, options));
    Some(guard)
}

#[derive(Default)]
struct SeenErrors {
    keys: HashSet<u64>,
}

impl SeenErrors {
    /// True the first time an error is seen this run, while under the cap.
    fn admit(&mut self, event: &Event<'_>) -> bool {
        self.keys.len() < MAX_EVENTS_PER_RUN && self.keys.insert(event_key(event))
    }
}

fn event_key(event: &Event<'_>) -> u64 {
    let mut hasher = DefaultHasher::new();
    event.message.hash(&mut hasher);
    for exception in &event.exception.values {
        exception.ty.hash(&mut hasher);
        exception.value.hash(&mut hasher);
        let top = exception
            .stacktrace
            .as_ref()
            .and_then(|trace| trace.frames.last());
        if let Some(frame) = top {
            frame.function.hash(&mut hasher);
            frame.lineno.hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// Removes everything a report must not carry.
fn scrub_event(mut event: Event<'static>) -> Event<'static> {
    event.user = None;
    event.request = None;
    event.server_name = None;
    event.breadcrumbs = Default::default();
    event.extra.clear();
    event.tags.clear();
    event.message = event.message.as_deref().map(scrub_text);
    if let Some(entry) = event.logentry.as_mut() {
        entry.message = scrub_text(&entry.message);
        entry.params.clear();
    }
    for exception in &mut event.exception.values {
        exception.value = exception.value.as_deref().map(scrub_text);
        if let Some(trace) = exception.stacktrace.as_mut() {
            for frame in &mut trace.frames {
                frame.vars.clear();
            }
        }
    }
    for thread in &mut event.threads.values {
        thread.name = None;
    }
    event
}

static QUOTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"`[^`]*`|"[^"]*"|'[^']{2,}'"#).unwrap());
static SECRETS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)\b(?:nsec|npub|nprofile|nevent|naddr|note)1[0-9a-z]{10,}",
        r"|[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}",
        r"|\b[0-9a-f]{32,}\b",
        r"|\b(?:bearer|token|key|secret|password|auth)\s*[:=]\s*\S+",
        r"|[A-Za-z0-9_\-]{40,}",
    ))
    .unwrap()
});
static URL_QUERY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b((?:https?|wss?)://[^\s?#]+)[?#]\S*").unwrap());

/// Scrubs one error string: quoted text, keys, tokens, emails and URL
/// queries are replaced, and the result is cut to [`MAX_TEXT_CHARS`].
pub(crate) fn scrub_text(text: &str) -> String {
    let text = QUOTED.replace_all(text, "[quoted]");
    let text = URL_QUERY.replace_all(&text, "$1");
    let text = SECRETS.replace_all(&text, "[redacted]");
    if text.chars().count() <= MAX_TEXT_CHARS {
        return text.into_owned();
    }
    text.chars().take(MAX_TEXT_CHARS).chain(['…']).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentry::protocol::{Breadcrumb, Exception, Request, User};

    #[test]
    fn scrub_text_removes_quoted_message_text() {
        let panic = "byte index 3 is not a char boundary; it is inside '가' (bytes 2..5) of `hello 가나다 secret plans`";
        let scrubbed = scrub_text(panic);
        assert!(!scrubbed.contains("secret plans"), "{scrubbed}");
        assert!(scrubbed.starts_with("byte index 3 is not a char boundary"));
    }

    #[test]
    fn scrub_text_removes_keys_tokens_and_emails() {
        let text = concat!(
            "failed for nsec1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq ",
            "pubkey 8cf9a7e7deebdc7d1fe172b9af4c6796b224e43e8f03e80dbd357f6c61fdeff9 ",
            "user someone@example.com token=abc.def.ghi ",
            "jwt eyJhbGciOiJIUzI1NiJ9eyJzdWIiOiIxMjM0NTY3ODkwIn0abcdefgh",
        );
        let scrubbed = scrub_text(text);
        for secret in [
            "nsec1",
            "8cf9a7e7",
            "someone@example.com",
            "abc.def.ghi",
            "eyJhbGci",
        ] {
            assert!(!scrubbed.contains(secret), "{secret} left in {scrubbed}");
        }
    }

    #[test]
    fn scrub_text_drops_url_queries_and_keeps_hosts() {
        let scrubbed = scrub_text("GET https://gateway.kuzz.app/api/x?auth=zzz#frag failed");
        assert_eq!(scrubbed, "GET https://gateway.kuzz.app/api/x failed");
    }

    #[test]
    fn scrub_text_caps_length() {
        let scrubbed = scrub_text(&"word ".repeat(200));
        assert_eq!(scrubbed.chars().count(), MAX_TEXT_CHARS + 1);
    }

    #[test]
    fn scrub_event_drops_identity_and_context() {
        let mut event = Event::new();
        event.user = Some(User {
            email: Some("someone@example.com".into()),
            ..Default::default()
        });
        event.request = Some(Request::default());
        event.server_name = Some("JIHO-PC".into());
        event.breadcrumbs.values.push(Breadcrumb::default());
        event.extra.insert("body".into(), "hello".into());
        event.tags.insert("pubkey".into(), "abc".into());
        event.exception.values.push(Exception {
            ty: "panic".into(),
            value: Some("called unwrap on `my message`".into()),
            ..Default::default()
        });

        let event = scrub_event(event);
        assert!(event.user.is_none());
        assert!(event.request.is_none());
        assert!(event.server_name.is_none());
        assert!(event.breadcrumbs.values.is_empty());
        assert!(event.extra.is_empty());
        assert!(event.tags.is_empty());
        assert_eq!(
            event.exception.values[0].value.as_deref(),
            Some("called unwrap on [quoted]")
        );
    }

    #[test]
    fn same_error_is_admitted_once_and_runs_are_capped() {
        let error = |value: &str| {
            let mut event = Event::new();
            event.exception.values.push(Exception {
                ty: "panic".into(),
                value: Some(value.into()),
                ..Default::default()
            });
            event
        };
        let mut seen = SeenErrors::default();
        assert!(seen.admit(&error("a")));
        assert!(!seen.admit(&error("a")));
        for n in 1..MAX_EVENTS_PER_RUN {
            assert!(seen.admit(&error(&n.to_string())));
        }
        assert!(!seen.admit(&error("one too many")));
    }
}
