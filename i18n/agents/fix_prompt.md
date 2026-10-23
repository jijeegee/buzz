You fix rejected UI translations. Read {REPO}/i18n/BRIEF.md and {REPO}/i18n/glossary/{LANG}.json. Then read {REPO}/i18n/work/{LANG}.json: every item there was rejected by the checker; each has `en`, `note`, and `problems`.

For each item write a translation into {LANG} that fixes the problem:
- "looks untranslated" (no letters of the language's script): write it with the language's own words/script when a natural translation exists (e.g. "Got it", "Ban", "Pulse" as a transliteration, "Kind {kind}", list joiners like "{a} and {b}" using the language's own "and"). Product/brand names may be transliterated. If the string truly must stay as-is (technical tokens like "NIP-05", "URL", "PID {pid}", "ACP: {command}", "{rate} tok/s" where no natural local word fits, placeholder examples like "your_id" or "party-parrot"), OMIT it from the output — it will show in English, which is fine.
- "keep \"Kuzz\" as is": the translation must contain the exact Latin word Kuzz.
- "say \"<Term>\" as the glossary does: a / b": the English uses that product word, so the translation must contain one of the listed forms (exact letters). Rephrase naturally around it; do not drop the word.
- Placeholder/tag/ICU problems: copy {placeholders}, <tags>, and plural/select structure exactly.

Write {REPO}/i18n/work/{LANG}.out.json as `{ "<id>": "<translation>" }` (valid UTF-8 JSON). Do not run apply. Reply with one line: how many written, how many omitted.
