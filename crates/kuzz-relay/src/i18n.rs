//! Text the relay writes for people (moderation notices, the group link
//! page) in their language.
//!
//! Strings live in `crates/kuzz-relay/locales/<lang>.json` (English is the
//! source; the others are written by the language-pack tooling in `i18n/`).
//! Every file in that folder is embedded at build time, so adding a language
//! needs no code change. A string missing from a language falls back to
//! English. Messages use the ICU MessageFormat subset the tooling validates:
//! `{name}`, `{n, plural, ...}` with `#`, and `{x, select, ...}`.

use std::collections::HashMap;
use std::sync::OnceLock;

use icu_locale_core::Locale;
use icu_plurals::{PluralCategory, PluralRules};

include!(concat!(env!("OUT_DIR"), "/locales.rs"));

/// The source language and fallback.
pub const SOURCE_LANG: &str = "en";

/// A placeholder value.
#[derive(Debug, Clone, Copy)]
pub enum Arg<'a> {
    /// Inserted as is.
    Str(&'a str),
    /// Inserted as a number; drives `plural`.
    Num(i64),
}

struct Language {
    code: String,
    rtl: bool,
}

struct Catalogs {
    /// Enabled languages that ship a relay file, in `languages.json` order.
    languages: Vec<Language>,
    strings: HashMap<String, HashMap<String, String>>,
}

fn catalogs() -> &'static Catalogs {
    static CATALOGS: OnceLock<Catalogs> = OnceLock::new();
    CATALOGS.get_or_init(|| {
        let mut strings = HashMap::new();
        for (lang, json) in LOCALE_FILES {
            let parsed: HashMap<String, serde_json::Value> =
                serde_json::from_str(json).unwrap_or_default();
            let messages = parsed
                .into_iter()
                .filter(|(key, _)| !key.starts_with('@'))
                .filter_map(|(key, value)| Some((key, value.as_str()?.to_owned())))
                .collect();
            strings.insert((*lang).to_owned(), messages);
        }
        let list: serde_json::Value = serde_json::from_str(LANGUAGES_JSON).unwrap_or_default();
        let languages = list["languages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|l| l["enabled"].as_bool().unwrap_or(false))
            .filter_map(|l| {
                let code = l["code"].as_str()?.to_owned();
                strings.contains_key(&code).then(|| Language {
                    rtl: l["dir"].as_str() == Some("rtl"),
                    code,
                })
            })
            .collect();
        Catalogs { languages, strings }
    })
}

/// Regional and legacy tags that map onto one of our codes (same rules as
/// the apps' `matchLocale` / `resolveAppLocale`).
fn alias(tag: &str) -> Option<&'static str> {
    Some(match tag {
        "zh-cn" | "zh-sg" | "zh-my" | "zh" => "zh-Hans",
        "zh-tw" | "zh-hk" | "zh-mo" => "zh-Hant",
        "pt" => "pt-BR",
        "tl" => "fil",
        "in" => "id",
        _ => return None,
    })
}

fn shipped(tag: &str) -> Option<&'static str> {
    catalogs()
        .languages
        .iter()
        .find(|l| l.code.eq_ignore_ascii_case(tag))
        .map(|l| l.code.as_str())
}

/// The first of `preferred` (most wanted first) the relay can write, else
/// English. `ko-KR` → `ko`, `zh-TW` → `zh-Hant`, `pt-PT` → `pt-BR`.
pub fn pick<'a>(preferred: impl IntoIterator<Item = &'a str>) -> &'static str {
    for tag in preferred {
        let tag = tag.trim().replace('_', "-").to_ascii_lowercase();
        let parts: Vec<&str> = tag.split('-').filter(|p| !p.is_empty()).collect();
        let candidates = (1..=parts.len()).rev().map(|n| parts[..n].join("-"));
        for candidate in candidates {
            let found = shipped(&candidate).or_else(|| alias(&candidate).and_then(shipped));
            if let Some(code) = found {
                return code;
            }
        }
    }
    SOURCE_LANG
}

/// Language for a browser request, from its `Accept-Language` header.
pub fn from_accept_language(header: Option<&str>) -> &'static str {
    let Some(header) = header else {
        return SOURCE_LANG;
    };
    let mut ranked: Vec<(f32, usize, &str)> = header
        .split(',')
        .enumerate()
        .filter_map(|(index, item)| {
            let mut parts = item.split(';');
            let tag = parts.next()?.trim();
            let q = parts
                .find_map(|p| p.trim().strip_prefix("q="))
                .and_then(|q| q.parse::<f32>().ok())
                .unwrap_or(1.0);
            (q > 0.0 && !tag.is_empty() && tag != "*").then_some((q, index, tag))
        })
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    pick(ranked.into_iter().map(|(_, _, tag)| tag))
}

/// `rtl` for Arabic, Urdu, Persian…; `ltr` otherwise.
pub fn direction(lang: &str) -> &'static str {
    let rtl = catalogs()
        .languages
        .iter()
        .any(|l| l.rtl && l.code.eq_ignore_ascii_case(lang));
    if rtl {
        "rtl"
    } else {
        "ltr"
    }
}

/// The string `key` in `lang` (English when missing) with `args` filled in.
pub fn tr(lang: &str, key: &str, args: &[(&str, Arg<'_>)]) -> String {
    let strings = &catalogs().strings;
    let message = strings
        .get(lang)
        .and_then(|m| m.get(key))
        .or_else(|| strings.get(SOURCE_LANG).and_then(|m| m.get(key)));
    match message {
        Some(message) => format_message(message, lang, args, None),
        None => key.to_owned(),
    }
}

fn plural_category(lang: &str, n: i64) -> PluralCategory {
    Locale::try_from_str(lang)
        .ok()
        .and_then(|locale| PluralRules::try_new_cardinal((&locale).into()).ok())
        .map(|rules| rules.category_for(n))
        .unwrap_or(if n == 1 {
            PluralCategory::One
        } else {
            PluralCategory::Other
        })
}

fn category_name(category: PluralCategory) -> &'static str {
    match category {
        PluralCategory::Zero => "zero",
        PluralCategory::One => "one",
        PluralCategory::Two => "two",
        PluralCategory::Few => "few",
        PluralCategory::Many => "many",
        PluralCategory::Other => "other",
    }
}

/// Index just past the `}` matching the `{` at `open`.
fn matching_brace(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in chars.iter().enumerate().skip(open) {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// `one {…} other {…}` → `[("one", "…"), ("other", "…")]`.
fn options(text: &str) -> Vec<(String, String)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let start = i;
        while i < chars.len() && chars[i] != '{' {
            i += 1;
        }
        let selector: String = chars[start..i].iter().collect::<String>().trim().to_owned();
        let Some(end) = matching_brace(&chars, i) else {
            break;
        };
        out.push((selector, chars[i + 1..end - 1].iter().collect()));
        i = end;
    }
    out
}

fn format_message(
    message: &str,
    lang: &str,
    args: &[(&str, Arg<'_>)],
    plural_value: Option<i64>,
) -> String {
    let arg = |name: &str| args.iter().find(|(n, _)| *n == name).map(|(_, a)| *a);
    let chars: Vec<char> = message.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '{' => {
                let Some(end) = matching_brace(&chars, i) else {
                    out.extend(&chars[i..]);
                    break;
                };
                let inner: String = chars[i + 1..end - 1].iter().collect();
                let mut parts = inner.splitn(3, ',');
                let name = parts.next().unwrap_or("").trim();
                match (parts.next().map(str::trim), parts.next()) {
                    (Some("plural"), Some(rest)) => {
                        let n = match arg(name) {
                            Some(Arg::Num(n)) => n,
                            _ => 0,
                        };
                        let opts = options(rest);
                        let exact = format!("={n}");
                        let category = category_name(plural_category(lang, n));
                        let chosen = opts
                            .iter()
                            .find(|(s, _)| *s == exact)
                            .or_else(|| opts.iter().find(|(s, _)| s == category))
                            .or_else(|| opts.iter().find(|(s, _)| s == "other"));
                        if let Some((_, body)) = chosen {
                            out.push_str(&format_message(body, lang, args, Some(n)));
                        }
                    }
                    (Some("select"), Some(rest)) => {
                        let value = match arg(name) {
                            Some(Arg::Str(s)) => s.to_owned(),
                            Some(Arg::Num(n)) => n.to_string(),
                            None => String::new(),
                        };
                        let opts = options(rest);
                        let chosen = opts
                            .iter()
                            .find(|(s, _)| *s == value)
                            .or_else(|| opts.iter().find(|(s, _)| s == "other"));
                        if let Some((_, body)) = chosen {
                            out.push_str(&format_message(body, lang, args, plural_value));
                        }
                    }
                    _ => match arg(name) {
                        Some(Arg::Str(s)) => out.push_str(s),
                        Some(Arg::Num(n)) => out.push_str(&n.to_string()),
                        None => out.push_str(&inner),
                    },
                }
                i = end;
            }
            '#' if plural_value.is_some() => {
                out.push_str(&plural_value.unwrap_or_default().to_string());
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_strings_fill_placeholders_and_plurals() {
        assert_eq!(
            tr(
                "en",
                "groups.landing.title",
                &[("name", Arg::Str("Hikers"))]
            ),
            "Hikers on Kuzz"
        );
        let members = |n| tr("en", "groups.landing.members", &[("count", Arg::Num(n))]);
        assert_eq!(members(1), "1 member");
        assert_eq!(members(3), "3 members");
    }

    #[test]
    fn missing_language_or_string_falls_back_to_english() {
        assert_eq!(tr("xx", "groups.landing.open", &[]), "Open in Kuzz");
        assert_eq!(tr("en", "no.such.key", &[]), "no.such.key");
    }

    #[test]
    fn plural_rules_follow_the_language() {
        let message =
            "{n, plural, =0 {none} one {# one} few {# few} many {# many} other {# other}}";
        let f = |lang, n| format_message(message, lang, &[("n", Arg::Num(n))], None);
        assert_eq!(f("en", 0), "none");
        assert_eq!(f("en", 1), "1 one");
        assert_eq!(f("en", 2), "2 other");
        assert_eq!(f("ru", 3), "3 few");
        assert_eq!(f("ru", 5), "5 many");
        assert_eq!(f("ko", 1), "1 other");
    }

    #[test]
    fn select_and_nested_text() {
        let message = "{kind, select, ban {Banned {name}} other {Other}}";
        assert_eq!(
            format_message(
                message,
                "en",
                &[("kind", Arg::Str("ban")), ("name", Arg::Str("Ann"))],
                None
            ),
            "Banned Ann"
        );
    }

    #[test]
    fn picks_shipped_languages_only() {
        // Only English ships until translations land.
        assert_eq!(pick(["ko-KR", "en-US"]), "en");
        assert_eq!(from_accept_language(Some("de;q=0.5, en-GB;q=0.9")), "en");
        assert_eq!(from_accept_language(None), "en");
        assert_eq!(direction("en"), "ltr");
    }
}
