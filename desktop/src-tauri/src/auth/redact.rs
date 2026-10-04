//! Redaction of Buzz bearer tokens (`bzl_ bzs_ bzr_ bzb_ bzk_`) from text that
//! can reach logs, `last_error`, or the UI.

use std::sync::OnceLock;

/// Replace every Buzz token in `text` with `[REDACTED]`.
pub(crate) fn redact_buzz_tokens(text: &str) -> std::borrow::Cow<'_, str> {
    static PATTERN: OnceLock<Option<regex::Regex>> = OnceLock::new();
    match PATTERN.get_or_init(|| regex::Regex::new(r"bz[lsrbk]_[A-Za-z0-9_-]{20,}").ok()) {
        Some(pattern) => pattern.replace_all(text, "[REDACTED]"),
        None => std::borrow::Cow::Borrowed(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_every_token_kind_and_keeps_the_rest() {
        let token = "Q0hBTkdFLU1FLXRoaXMtaXMtYS10ZXN0LXRva2Vu";
        for prefix in ["bzl_", "bzs_", "bzr_", "bzb_", "bzk_"] {
            let line = format!("auth failed for {prefix}{token} at relay");
            let redacted = redact_buzz_tokens(&line);
            assert!(!redacted.contains(token), "{prefix}: {redacted}");
            assert_eq!(redacted, "auth failed for [REDACTED] at relay");
        }
        assert_eq!(redact_buzz_tokens("bzs_short ok"), "bzs_short ok");
        assert_eq!(redact_buzz_tokens("no secrets"), "no secrets");
    }

    /// The managed-agent redaction every log/`last_error` path uses also
    /// scrubs Buzz tokens (removing the hook fails this).
    #[test]
    fn managed_agent_redaction_scrubs_buzz_tokens() {
        let token = format!("bzb_{}", "x".repeat(43));
        let scrubbed =
            crate::managed_agents::redact_secrets_with(&format!("token={token} expired"), &[]);
        assert!(!scrubbed.contains(&token), "{scrubbed}");
        assert!(scrubbed.contains("[REDACTED]"));
    }
}
