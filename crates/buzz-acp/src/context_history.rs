//! Owner-chosen amount of conversation history a fresh session reads.
//!
//! The desktop passes the choice in `BUZZ_ACP_CONTEXT_HISTORY` as `small`,
//! `medium`, or `large`; anything else (including unset) keeps the default of
//! the most recent `context_message_limit` messages. A budget applies only to
//! the first `<thread-context>` / `<conversation-context>` fetch of a thread or
//! main-timeline scope in a session this harness created: a reattached
//! (resumed) session already holds the earlier conversation, and later turns
//! receive only the delivery delta, so both keep the recent-N window.
//! Dispatchers always keep their own small window (see `Config`).

use std::str::FromStr;

use crate::queue::{ContextMessage, ConversationContext};

/// Replies requested from the relay for a budgeted thread fetch. One more is
/// requested as the truncation sentinel, which keeps the request at the
/// relay's page cap (`DEFAULT_MAX_PAGE_LIMIT`, 500). The main timeline is
/// further clamped to its own window cap by the fetcher.
pub const HISTORY_FETCH_LIMIT: u32 = 499;

/// Estimated per-message prompt overhead: the `[n] Name (timestamp): ` line
/// prefix rendered by `format_conversation_context`.
const MESSAGE_OVERHEAD_TOKENS: usize = 16;

/// How much history a fresh session reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ContextHistory {
    /// The most recent `context_message_limit` messages (the default).
    #[default]
    Recent,
    /// As much of the conversation as fits in the budget.
    Budget(ContextBudget),
}

/// Size budgets for [`ContextHistory::Budget`], in estimated tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextBudget {
    Small,
    Medium,
    Large,
}

impl ContextBudget {
    /// Estimated prompt tokens the history may use. Sized so even Large stays
    /// a modest share of a 200k-token context window.
    pub const fn tokens(self) -> usize {
        match self {
            Self::Small => 8_000,
            Self::Medium => 20_000,
            Self::Large => 50_000,
        }
    }
}

impl FromStr for ContextHistory {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "recent" => Ok(Self::Recent),
            "small" => Ok(Self::Budget(ContextBudget::Small)),
            "medium" => Ok(Self::Budget(ContextBudget::Medium)),
            "large" => Ok(Self::Budget(ContextBudget::Large)),
            other => Err(format!("unknown context history: {other}")),
        }
    }
}

/// Parse the env value. An unknown value keeps the default with a warning so
/// a newer desktop never stops an older harness from starting.
pub fn parse_context_history(raw: Option<&str>) -> ContextHistory {
    raw.unwrap_or_default().parse().unwrap_or_else(|error| {
        tracing::warn!("ignoring BUZZ_ACP_CONTEXT_HISTORY: {error}");
        ContextHistory::Recent
    })
}

impl ContextHistory {
    /// The budget for this fetch, or `None` to keep the recent-N window.
    /// Only a scope's first fetch in a session this harness created reads
    /// beyond recent N: `hydrated` means the session already received this
    /// thread's or main timeline's context, and `reattached` means the
    /// provider resumed a session that already holds the conversation.
    pub fn budget_for_fetch(self, hydrated: bool, reattached: bool) -> Option<ContextBudget> {
        match self {
            Self::Budget(budget) if !hydrated && !reattached => Some(budget),
            _ => None,
        }
    }
}

/// Rough token estimate that holds for mixed-script text: about four ASCII
/// characters per token, one token per non-ASCII character (CJK, emoji).
pub fn estimate_tokens(text: &str) -> usize {
    let (ascii, other) = text.chars().fold((0usize, 0usize), |(a, o), c| {
        if c.is_ascii() {
            (a + 1, o)
        } else {
            (a, o + 1)
        }
    });
    ascii.div_ceil(4) + other
}

fn message_tokens(message: &ContextMessage) -> usize {
    estimate_tokens(&message.content) + MESSAGE_OVERHEAD_TOKENS
}

/// Trim a chronologically ordered context to `budget_tokens`.
///
/// The newest `keep_recent` messages and a thread's root are always kept, so
/// a budget never shows less than the recent-N default. Older messages are
/// then added newest-first while they fit; the first one that does not fit
/// ends the window so the kept history stays contiguous. Dropping anything
/// marks the context truncated; `total` keeps the fetched count.
pub fn apply_context_budget(
    context: ConversationContext,
    keep_recent: usize,
    budget_tokens: usize,
) -> ConversationContext {
    let trim = |messages: Vec<ContextMessage>, has_root: bool| {
        let (root, rest) = if has_root && !messages.is_empty() {
            let mut messages = messages;
            let rest = messages.split_off(1);
            (messages.pop(), rest)
        } else {
            (None, messages)
        };
        let mut used = root.as_ref().map_or(0, message_tokens);
        let mut keep_from = rest.len();
        for (index, message) in rest.iter().enumerate().rev() {
            let cost = message_tokens(message);
            let within_recent = rest.len() - index <= keep_recent;
            if !within_recent && used + cost > budget_tokens {
                break;
            }
            used += cost;
            keep_from = index;
        }
        let dropped = keep_from > 0;
        let mut kept: Vec<ContextMessage> = root.into_iter().collect();
        kept.extend(rest.into_iter().skip(keep_from));
        (kept, dropped)
    };

    match context {
        ConversationContext::Thread {
            messages,
            total,
            root_present,
            truncated,
        } => {
            let (messages, dropped) = trim(messages, root_present);
            ConversationContext::Thread {
                messages,
                total,
                root_present,
                truncated: truncated || dropped,
            }
        }
        ConversationContext::Dm {
            messages,
            total,
            truncated,
        } => {
            let (messages, dropped) = trim(messages, false);
            ConversationContext::Dm {
                messages,
                total,
                truncated: truncated || dropped,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(id: &str, content: &str) -> ContextMessage {
        ContextMessage {
            event_id: id.into(),
            pubkey: "ab".repeat(32),
            timestamp: "2026-10-08T00:00:00Z".into(),
            content: content.into(),
        }
    }

    /// `n` replies of `chars` ASCII characters each, oldest first.
    fn replies(n: usize, chars: usize) -> Vec<ContextMessage> {
        (0..n)
            .map(|i| message(&format!("r{i}"), &"x".repeat(chars)))
            .collect()
    }

    fn ids(context: &ConversationContext) -> Vec<String> {
        match context {
            ConversationContext::Thread { messages, .. }
            | ConversationContext::Dm { messages, .. } => {
                messages.iter().map(|m| m.event_id.clone()).collect()
            }
        }
    }

    fn truncated(context: &ConversationContext) -> bool {
        match context {
            ConversationContext::Thread { truncated, .. }
            | ConversationContext::Dm { truncated, .. } => *truncated,
        }
    }

    #[test]
    fn parses_known_values_and_falls_back_to_recent() {
        assert_eq!(parse_context_history(None), ContextHistory::Recent);
        assert_eq!(parse_context_history(Some("")), ContextHistory::Recent);
        assert_eq!(
            parse_context_history(Some("recent")),
            ContextHistory::Recent
        );
        assert_eq!(
            parse_context_history(Some(" Medium ")),
            ContextHistory::Budget(ContextBudget::Medium)
        );
        assert_eq!(
            parse_context_history(Some("large")),
            ContextHistory::Budget(ContextBudget::Large)
        );
        assert_eq!(parse_context_history(Some("huge")), ContextHistory::Recent);
        assert!(ContextBudget::Small.tokens() < ContextBudget::Medium.tokens());
        assert!(ContextBudget::Medium.tokens() < ContextBudget::Large.tokens());
    }

    #[test]
    fn only_a_fresh_sessions_first_fetch_gets_the_budget() {
        let medium = ContextHistory::Budget(ContextBudget::Medium);
        assert_eq!(
            medium.budget_for_fetch(false, false),
            Some(ContextBudget::Medium)
        );
        assert_eq!(medium.budget_for_fetch(true, false), None, "hydrated");
        assert_eq!(medium.budget_for_fetch(false, true), None, "resumed");
        assert_eq!(ContextHistory::Recent.budget_for_fetch(false, false), None);
    }

    #[test]
    fn estimates_ascii_and_wide_text() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(estimate_tokens("안녕하세요"), 5);
    }

    #[test]
    fn history_that_fits_is_kept_whole() {
        let mut messages = vec![message("root", "start")];
        messages.extend(replies(40, 40));
        let context = ConversationContext::Thread {
            messages,
            total: 41,
            root_present: true,
            truncated: false,
        };
        let trimmed = apply_context_budget(context, 12, 8_000);
        assert_eq!(ids(&trimmed).len(), 41);
        assert!(!truncated(&trimmed));
    }

    #[test]
    fn over_budget_drops_oldest_and_keeps_the_root() {
        // Each reply costs 25 + 16 = 41 tokens; the root costs 2 + 16 = 18.
        let mut messages = vec![message("root", "start")];
        messages.extend(replies(30, 100));
        let context = ConversationContext::Thread {
            messages,
            total: 31,
            root_present: true,
            truncated: false,
        };
        let trimmed = apply_context_budget(context, 2, 18 + 41 * 10);
        let kept = ids(&trimmed);
        assert_eq!(kept.len(), 11);
        assert_eq!(kept[0], "root");
        assert_eq!(kept[1], "r20", "oldest replies are dropped first");
        assert_eq!(kept[10], "r29", "the newest reply is kept");
        assert!(truncated(&trimmed));
        let ConversationContext::Thread { total, .. } = trimmed else {
            panic!("thread stays thread");
        };
        assert_eq!(total, 31, "total keeps the fetched count");
    }

    #[test]
    fn recent_window_survives_a_tiny_budget() {
        let context = ConversationContext::Dm {
            messages: replies(20, 4_000),
            total: 20,
            truncated: false,
        };
        let trimmed = apply_context_budget(context, 12, 100);
        let kept = ids(&trimmed);
        assert_eq!(kept.len(), 12, "never fewer than the recent-N default");
        assert_eq!(kept.first().map(String::as_str), Some("r8"));
        assert_eq!(kept.last().map(String::as_str), Some("r19"));
        assert!(truncated(&trimmed));
    }

    #[test]
    fn kept_history_is_contiguous() {
        // r0 would still fit after r1 is dropped, but keeping it would leave
        // a gap, so the window ends at the first message that does not fit.
        let messages = vec![
            message("r0", "y"),
            message("r1", &"x".repeat(4_000)),
            message("r2", "z"),
        ];
        let context = ConversationContext::Dm {
            messages,
            total: 3,
            truncated: false,
        };
        let trimmed = apply_context_budget(context, 1, 100);
        assert_eq!(ids(&trimmed), vec!["r2".to_string()]);
    }
}
