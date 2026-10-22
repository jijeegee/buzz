//! Route test for device-bound keys (plan §3, PR A2): every function that
//! verifies a message key's signature also admits the key on its device, and
//! every token verification on a message path goes through
//! [`super::verify_message_token`]. A new entrance fails here until it is
//! wired or listed below with a reason.
//!
//! The scan is per function body: comments, string and char literals and
//! `#[cfg(test)]` items are blanked, each call site is attributed to its
//! innermost `fn`, and definitions (`fn verify_...`) are not calls. Only
//! `kuzz-relay/src` is scanned; entrances in other crates are exceptions by
//! construction (the push gateway, plan §3: documented exception).

use std::path::{Path, PathBuf};

/// Calls that verify a message key's signature, by name prefix: NIP-42,
/// NIP-98 (and the bridge helpers around it), Blossom, NIP-98 headers.
const VERIFIER_PREFIXES: &[&str] = &[
    "verify_auth_event",
    "verify_nip42_event",
    "verify_nip98",
    "verify_bridge_auth",
    "verify_blossom",
    "verify_auth_header",
    "verify_signature",
];

/// Calls that verify a key signature, by exact name: the NIP-FI HTTP choke
/// point, its NIP-98 closure, Blossom extraction, link proofs and helpers
/// that verify for their callers.
const VERIFIER_NAMES: &[&str] = &[
    "admit_nip_fi_http_on_state",
    "make_nip98_closure_for_admission",
    "extract_blossom_auth",
    "extract_blossom_read_proof",
    "validate_link_proof",
    "authorize_nip98",
];

/// Calls that admit a key on its device.
const ADMISSION_NAMES: &[&str] = &[
    "admit_key",
    "admit_key_with_owner",
    "admit_key_socket",
    "admit_key_for_event",
    "admit_key_for_nip98",
    "admit_bridge_key",
    "admit_media_key",
    "admit_admin_key",
    "admit_operator_key",
];

/// Why a function that verifies a key does not admit it itself.
enum Why {
    /// A helper that verifies for its callers; its own name is a verifier
    /// name above, so every caller is checked in turn.
    Helper,
    /// Admission happens in this callee (after the replay check), which
    /// must call it and must admit.
    Delegated(&'static str),
    /// Not a message key entrance.
    Exempt(&'static str),
}

const LEASE: &str = "serving lease check (`serving_write.verify()`), not a signature";

/// (file, function, why) for verifier calls without admission.
const KEY_EXCEPTIONS: &[(&str, &str, Why)] = &[
    ("api/admin/auth.rs", "authorize_nip98", Why::Helper),
    (
        "api/auth/agents.rs",
        "status",
        Why::Exempt(
            "reads key_admission itself and answers only an admitted caller, in log \
             mode too (stricter than admit_key); off returns no keys",
        ),
    ),
    (
        "api/auth/key_backup.rs",
        "initialize",
        Why::Exempt(
            "link proof: a signed-in account (Bearer session) proves it holds the key it \
             registers; the key is the subject, not the caller",
        ),
    ),
    (
        "api/bridge.rs",
        "submit_event",
        Why::Delegated("submit_event_authed"),
    ),
    (
        "api/bridge.rs",
        "query_events",
        Why::Delegated("query_events_authed"),
    ),
    (
        "api/bridge.rs",
        "count_events",
        Why::Delegated("count_events_authed"),
    ),
    (
        "api/bridge.rs",
        "make_nip98_closure_for_admission",
        Why::Helper,
    ),
    ("api/bridge.rs", "verify_bridge_auth", Why::Helper),
    (
        "api/bridge.rs",
        "verify_bridge_auth_with_options",
        Why::Helper,
    ),
    (
        "api/bridge.rs",
        "verify_nip98_exempt_invite_claim",
        Why::Helper,
    ),
    ("api/bridge.rs", "verify_nip98_exempt_operator", Why::Helper),
    ("api/media.rs", "extract_blossom_read_proof", Why::Helper),
    ("api/media.rs", "upload_blob_result", Why::Exempt(LEASE)),
    (
        "api/git/settings.rs",
        "set_default_branch",
        Why::Exempt(LEASE),
    ),
    (
        "api/git/transport.rs",
        "finalize_push_inner",
        Why::Exempt(LEASE),
    ),
    (
        "handlers/side_effects.rs",
        "handle_git_repo_announcement_inner",
        Why::Exempt(LEASE),
    ),
    ("push_runtime.rs", "deliver_one", Why::Exempt(LEASE)),
    (
        "api/nip_fi.rs",
        "disconnect",
        Why::Exempt("NIP-FI command JWT from a configured issuer, not a message key"),
    ),
    (
        "identity/device.rs",
        "verify_proof",
        Why::Exempt("the P-256 device proof of account calls (surface `auth`)"),
    ),
];

/// (file, function, why) for direct `verify_access_token` calls; message
/// paths must use `verify_message_token`.
const TOKEN_EXCEPTIONS: &[(&str, &str, &str)] = &[
    (
        "identity/mod.rs",
        "verify_message_token",
        "the message-path wrapper itself",
    ),
    (
        "api/auth/mod.rs",
        "authenticate",
        "/auth/* account calls (me, logout, devices, key backup), not a message path",
    ),
    (
        "api/admin/auth.rs",
        "authorize_bearer",
        "admin console Bearer session, an account call",
    ),
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        let name = path
            .file_name()
            .expect("file name")
            .to_string_lossy()
            .into_owned();
        if path.is_dir() {
            // Live tests sign with test keys on purpose.
            if name != "live_tests" {
                rust_files(&path, out);
            }
        } else if name.ends_with(".rs")
            // Test-only modules (declared under `#[cfg(test)]`).
            && !name.ends_with("tests.rs")
            && !matches!(
                name.as_str(),
                "test_support.rs" | "nip_fi_test_hooks.rs" | "admission_routes.rs"
            )
        {
            out.push(path);
        }
    }
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// `src` with comments and the contents of string and char literals replaced
/// by spaces (newlines kept), so braces and names in them do not count.
fn strip(src: &str) -> Vec<u8> {
    let s = src.as_bytes();
    let mut out = s.to_vec();
    let blank = |out: &mut Vec<u8>, from: usize, to: usize| {
        for byte in &mut out[from..to] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    };
    let n = s.len();
    let mut i = 0;
    while i < n {
        let rest = &s[i..];
        if rest.starts_with(b"//") {
            let end = s[i..].iter().position(|&b| b == b'\n').map_or(n, |p| i + p);
            blank(&mut out, i, end);
            i = end;
        } else if rest.starts_with(b"/*") {
            let (mut depth, mut j) = (1, i + 2);
            while j < n && depth > 0 {
                if s[j..].starts_with(b"/*") {
                    depth += 1;
                    j += 2;
                } else if s[j..].starts_with(b"*/") {
                    depth -= 1;
                    j += 2;
                } else {
                    j += 1;
                }
            }
            blank(&mut out, i, j);
            i = j;
        } else if (s[i] == b'r' || s[i] == b'b') && (i == 0 || !is_ident(s[i - 1])) {
            // Raw strings r"..", r#".."#, br".."; byte strings b"..".
            let mut j = i + usize::from(s[i] == b'b');
            if j < n && s[j] == b'r' {
                let mut k = j + 1;
                let mut hashes = 0;
                while k < n && s[k] == b'#' {
                    hashes += 1;
                    k += 1;
                }
                if k < n && s[k] == b'"' {
                    let mut close = vec![b'"'];
                    close.extend(std::iter::repeat_n(b'#', hashes));
                    let end = s[k + 1..]
                        .windows(close.len())
                        .position(|w| w == close.as_slice())
                        .map_or(n, |p| k + 1 + p);
                    blank(&mut out, k + 1, end);
                    i = (end + close.len()).min(n);
                    continue;
                }
            }
            j = i + 1;
            i = if s[i] == b'b' && j < n && (s[j] == b'"' || s[j] == b'\'') {
                j
            } else {
                i + 1
            };
        } else if s[i] == b'"' {
            let mut j = i + 1;
            while j < n && s[j] != b'"' {
                j += if s[j] == b'\\' { 2 } else { 1 };
            }
            blank(&mut out, i + 1, j.min(n));
            i = j + 1;
        } else if s[i] == b'\'' {
            // A char literal ('x', '\n', '\u{..}', a multi-byte char), not a
            // lifetime ('a).
            if i + 1 < n && s[i + 1] == b'\\' {
                let end = s[i + 2..]
                    .iter()
                    .position(|&b| b == b'\'')
                    .map_or(n, |p| i + 2 + p);
                blank(&mut out, i, (end + 1).min(n));
                i = end + 1;
            } else if let Some(ch) = src[i + 1..].chars().next() {
                let after = i + 1 + ch.len_utf8();
                if after < n && s[after] == b'\'' {
                    blank(&mut out, i, after + 1);
                    i = after + 1;
                } else {
                    i += 1;
                }
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

fn matching_brace(s: &[u8], open: usize) -> usize {
    let mut depth = 0usize;
    for (offset, &byte) in s[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return open + offset;
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced braces");
}

/// Blank every `#[cfg(test)]` item that has a body (a test module or fn);
/// attributes on fields, statements and `mod x;` are left alone.
fn drop_test_items(s: &mut [u8]) {
    const ATTR: &[u8] = b"#[cfg(test)]";
    let mut from = 0;
    while let Some(at) = s[from..]
        .windows(ATTR.len())
        .position(|w| w == ATTR)
        .map(|p| from + p)
    {
        let next = s[at..]
            .iter()
            .position(|&b| matches!(b, b'{' | b';' | b',' | b'}'))
            .map(|p| at + p);
        if let Some(open) = next.filter(|&p| s[p] == b'{') {
            let close = matching_brace(s, open);
            for byte in &mut s[at..=close] {
                if *byte != b'\n' {
                    *byte = b' ';
                }
            }
        }
        from = at + 1;
    }
}

/// (name, body open, body close) of every `fn` with a body.
fn functions(s: &[u8]) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    for at in 0..s.len().saturating_sub(3) {
        if &s[at..at + 3] != b"fn " || (at > 0 && is_ident(s[at - 1])) {
            continue;
        }
        let start = at + 3;
        let end = start + s[start..].iter().take_while(|&&b| is_ident(b)).count();
        let name = String::from_utf8_lossy(&s[start..end]).into_owned();
        // The body is the first `{` outside (..) and [..] (`[u8; 32]`).
        let mut depth = 0i32;
        let mut open = None;
        for (offset, &byte) in s[end..].iter().enumerate() {
            match byte {
                b'(' | b'[' => depth += 1,
                b')' | b']' => depth -= 1,
                b';' if depth == 0 => break,
                b'{' if depth == 0 => {
                    open = Some(end + offset);
                    break;
                }
                _ => {}
            }
        }
        if let Some(open) = open {
            out.push((name, open, matching_brace(s, open)));
        }
    }
    out
}

/// Call sites (position, name) of identifiers matching `hit`: followed by
/// `(`, not preceded by `fn `.
fn call_sites(s: &[u8], hit: impl Fn(&str) -> bool) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if is_ident(s[i]) && (i == 0 || !is_ident(s[i - 1])) {
            let end = i + s[i..].iter().take_while(|&&b| is_ident(b)).count();
            let word = std::str::from_utf8(&s[i..end]).unwrap_or_default();
            let next = s[end..].iter().find(|b| !b.is_ascii_whitespace());
            let definition = i >= 3 && &s[i - 3..i] == b"fn ";
            if hit(word) && next == Some(&b'(') && !definition {
                out.push((i, word.to_owned()));
            }
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

/// `.verify(` method calls (event signatures, and lease checks to except).
fn method_verify_sites(s: &[u8]) -> Vec<(usize, String)> {
    s.windows(b".verify(".len())
        .enumerate()
        .filter(|(_, w)| *w == b".verify(")
        .map(|(at, _)| (at, ".verify".to_owned()))
        .collect()
}

#[derive(Default)]
struct FnHits {
    verifiers: Vec<String>,
    admissions: usize,
    tokens: usize,
    calls: Vec<String>,
}

#[test]
fn every_key_and_token_entrance_is_admitted_or_excepted() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    files.sort();

    // (file, fn) → hits.
    let mut hits: std::collections::BTreeMap<(String, String), FnHits> = Default::default();
    let mut outside = Vec::new();
    for path in &files {
        let rel = path
            .strip_prefix(&src)
            .expect("under src")
            .to_string_lossy()
            .replace('\\', "/");
        let mut code = strip(&std::fs::read_to_string(path).expect("read source"));
        drop_test_items(&mut code);
        let fns = functions(&code);
        let owner = |at: usize| {
            fns.iter()
                .filter(|(_, open, close)| *open <= at && at <= *close)
                .max_by_key(|(_, open, _)| *open)
                .map(|(name, ..)| name.clone())
        };
        let verifier = |word: &str| {
            VERIFIER_PREFIXES.iter().any(|p| word.starts_with(p)) || VERIFIER_NAMES.contains(&word)
        };
        let mut record = |at: usize, what: &str, apply: &dyn Fn(&mut FnHits)| match owner(at) {
            Some(name) => apply(hits.entry((rel.clone(), name)).or_default()),
            None => outside.push(format!("{rel}: {what} outside any fn")),
        };
        for (at, word) in call_sites(&code, verifier)
            .into_iter()
            .chain(method_verify_sites(&code))
        {
            record(at, &word, &|h| h.verifiers.push(word.clone()));
        }
        for (at, word) in call_sites(&code, |w| ADMISSION_NAMES.contains(&w)) {
            record(at, &word, &|h| h.admissions += 1);
        }
        for (at, word) in call_sites(&code, |w| w == "verify_access_token") {
            record(at, &word, &|h| h.tokens += 1);
        }
        // Calls to the delegation targets, for `Why::Delegated`.
        for (at, word) in call_sites(&code, |w| {
            KEY_EXCEPTIONS
                .iter()
                .any(|(_, _, why)| matches!(why, Why::Delegated(callee) if *callee == w))
        }) {
            record(at, &word, &|h| h.calls.push(word.clone()));
        }
    }
    assert!(outside.is_empty(), "scanner lost calls: {outside:#?}");

    let mut unwired = Vec::new();
    for ((file, function), found) in &hits {
        if !found.verifiers.is_empty() && found.admissions == 0 {
            match KEY_EXCEPTIONS
                .iter()
                .find(|(f, n, _)| f == file && n == function)
            {
                None => unwired.push(format!(
                    "{file}::{function} verifies {:?} without admit_key",
                    found.verifiers
                )),
                Some((_, _, Why::Helper)) => assert!(
                    VERIFIER_PREFIXES.iter().any(|p| function.starts_with(p))
                        || VERIFIER_NAMES.contains(&function.as_str()),
                    "helper {file}::{function} must be a verifier name so its callers are checked"
                ),
                Some((_, _, Why::Delegated(callee))) => {
                    assert!(
                        found.calls.iter().any(|c| c == callee),
                        "{file}::{function} must call {callee}"
                    );
                    let admits = hits
                        .get(&(file.clone(), (*callee).to_owned()))
                        .is_some_and(|h| h.admissions > 0);
                    assert!(admits, "{file}::{callee} must admit the key");
                }
                Some((_, _, Why::Exempt(reason))) => assert!(!reason.is_empty()),
            }
        }
        if found.tokens > 0
            && !TOKEN_EXCEPTIONS
                .iter()
                .any(|(f, n, _)| f == file && n == function)
        {
            unwired.push(format!(
                "{file}::{function}: verify_access_token on a message path (use verify_message_token)"
            ));
        }
    }
    assert!(unwired.is_empty(), "unwired entrances: {unwired:#?}");

    // Stale exceptions hide nothing; drop them.
    for (file, function, _) in KEY_EXCEPTIONS {
        let key = ((*file).to_owned(), (*function).to_owned());
        assert!(
            hits.get(&key).is_some_and(|h| !h.verifiers.is_empty()),
            "stale key exception {file}::{function}"
        );
    }
    for (file, function, reason) in TOKEN_EXCEPTIONS {
        assert!(!reason.is_empty());
        let key = ((*file).to_owned(), (*function).to_owned());
        assert!(
            hits.get(&key).is_some_and(|h| h.tokens > 0),
            "stale token exception {file}::{function}"
        );
    }
    // The scan must see the known entrances at all.
    for (file, function) in [
        ("handlers/auth.rs", "handle_auth"),
        ("audio/handler.rs", "handle_active_audio_connection"),
        ("api/media.rs", "admit_blossom_upload"),
        ("api/media.rs", "authorize_blossom_read"),
        ("api/git/transport.rs", "from_request_parts"),
        ("api/bridge.rs", "authorize_moderation_read"),
    ] {
        let found = hits.get(&(file.to_owned(), function.to_owned()));
        assert!(
            found.is_some_and(|h| !h.verifiers.is_empty() && h.admissions > 0),
            "scan missed {file}::{function}"
        );
    }
}

/// The scanner itself: literals, comments and test items do not count, a
/// definition is not a call, and a `[u8; 32]` signature still finds its body.
#[test]
fn the_scanner_reads_function_bodies() {
    let src = r##"
fn wired(key: &[u8; 32]) {
    let _ = "fn fake() { verify_auth_event(x) }";
    let _ = '{';
    // verify_nip98_event(x)
    verify_auth_event(x);
    admit_key(x);
}
fn verify_nip98_helper() {}
fn bare() -> Result<(), ()> { let _ = r#"}"#; verify_nip98_event(x) }
#[cfg(test)]
mod tests { fn t() { verify_auth_event(x); } }
"##;
    let mut code = strip(src);
    drop_test_items(&mut code);
    let fns = functions(&code);
    let names: Vec<_> = fns.iter().map(|(name, ..)| name.as_str()).collect();
    assert_eq!(names, ["wired", "verify_nip98_helper", "bare"]);
    let sites = call_sites(&code, |w| w.starts_with("verify_"));
    let owners: Vec<_> = sites
        .iter()
        .map(|(at, _)| {
            fns.iter()
                .find(|(_, open, close)| open <= at && at <= close)
                .map(|(name, ..)| name.as_str())
        })
        .collect();
    assert_eq!(owners, [Some("wired"), Some("bare")]);
}
