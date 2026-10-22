# Language packs

English is the source. Every other language is a file an AI agent writes and
a script checks. Nothing here calls an API: translation happens when a release
is prepared, not at runtime.

## Files

| Path | What |
|---|---|
| `languages.json` | Languages: code, native name, direction, scripts used, `enabled`. Add or disable a language here only; apps read this list. |
| `sources.json` | Where each app keeps its strings (`{lang}` = `pt-BR`, `{lang_}` = `pt_BR`). |
| `BRIEF.md` | What every translator agent reads first. Keep it under 1,000 characters. |
| `glossary/terms.json` | Product words and what they mean. |
| `glossary/<lang>.json` | How those words are said in one language. Created on that language's first translation, then fixed. |
| `state/<lang>.json` | Hash of the English each translation was made from. Script-owned. |
| `work/` | Temporary files for one translation run. Not committed. |

## Writing English strings

Each app's English file holds the text plus a note on where it appears:

```json
"chats.empty.title": "No chats yet",
"@chats.empty.title": { "description": "Empty Chats list, big title. Short." },
"room.members": "{count, plural, one {# member} other {# members}}",
"@room.members": { "description": "Room header, small grey text after the name" }
```

- Name keys by screen: `settings.notifications.title` (desktop, web, relay); mobile `.arb` keys must be Dart names, so `settingsNotificationsTitle`.
- In code: desktop `const t = useT(); t("nav.chats")` (or `t()` from `@/shared/i18n` outside React); mobile `context.l10n.navChats` (or `appL10n` without a context).
- The note says where (screen, place) and what (button, title, error, notification). Add "Short." when space is tight.
- Use ICU plurals instead of `n === 1 ? ... : ...`, and whole sentences with `{placeholders}` instead of joining pieces.
- Do not use literal `{` `}` in text. Apostrophes are plain text.

`node i18n/scripts/i18n.mjs check` (part of `just check`) fails on invalid ICU, a missing note, or a translation that breaks its placeholders.

## Updating translations (before a release)

1. `node i18n/scripts/i18n.mjs prepare` writes `i18n/work/<lang>.json` for each language with new or changed English only. Each item has the English, its note, the old translation when the English changed (`previous`), and a translation of the same sentence from another app when one exists (`suggestion`; reuse it unless the note calls for different wording).
2. Translate with subagents, a few languages each. Give every subagent `BRIEF.md`, `glossary/<lang>.json` and the work file, and nothing else about the codebase. It writes `i18n/work/<lang>.out.json` as `{ "<id>": "<translation>" }`.
   - If the work file has `glossaryTerms` (no glossary yet), first write `glossary/<lang>.json` mapping each term to the word that language's popular messengers use (Telegram's and Instagram's official translations when unsure).
3. `node i18n/scripts/i18n.mjs apply` checks each translation and writes the language files. Rejected items stay in the work file with `problems`; send only those back to the same subagent, then apply again. After two failed tries, leave the item: it shows in English.
4. Back-translation check: a different subagent translates the new strings back into English from the language files alone. Re-translate only the ones whose meaning changed.
5. `node i18n/scripts/i18n.mjs check`, then open one PR with the counts per language.

People may fix any translation directly in a language file. The script only re-asks a string when its English changes, so manual fixes stay.

## Adding a language

Add one entry to `languages.json` (`script` lists the Unicode scripts its letters use, empty for Latin), run steps 1–5, and the apps pick it up. If a language turns out broken, set `"enabled": false`: it disappears from the picker and its users see English.

## Pseudo language

`node i18n/scripts/i18n.mjs pseudo` writes `en-XA`: every string accented and about 40% longer. Run the app in it to spot text that was never moved into a language file and labels that overflow. These files are not committed.
