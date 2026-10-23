# Language packs

English is the source. Every other language is a file an AI agent writes and
a script checks. Nothing here calls an API: translation happens when a release
is prepared, not at runtime.

To run a round with AI sub-agents (rules, commands, costs, lessons), follow
[`agents/RUNBOOK.md`](agents/RUNBOOK.md).

## Files

| Path | What |
|---|---|
| `languages.json` | Languages: code, native name, direction, scripts used, `enabled`. Add or disable a language here only; apps read this list. |
| `sources.json` | Where each app keeps its strings (`{lang}` = `pt-BR`, `{lang_}` = `pt_BR`). |
| `BRIEF.md` | What every translator agent reads first. Keep it under 1,000 characters. |
| `glossary/terms.json` | Product words, what they mean, and `fixed`: whether every translation must use the glossary word. |
| `glossary/<lang>.json` | How those words are said in one language: one word, or a list of accepted forms (preferred first, then stems that appear in every grammatical form). Created on that language's first translation, then kept. |
| `tiers.json` | Which text is translated in which release round (`current` = highest open tier; `skip` = stays English, e.g. admin and developer screens). |
| `later/<lang>.json` | Translations of text above the current tier, kept aside so every language shows the same English. Script-owned. |
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
- Reuse `common.*` (Cancel, Save, Delete, Close, …) for plain buttons; give a screen its own key when the word needs a different note.
- Never call `t()` / `appL10n` at module load (top-level constants): it would freeze English. Translate inside the function or component, or keep keys in the constant and translate where shown.
- Leave out what users never read as UI text: logs, internal errors, test ids, protocol values, and the name Kuzz inside other words' keys.
- Links or bold inside a sentence: desktop `t("x", { b: (s) => <b>{s}</b> })` with `"<b>bold</b>"` in the text; keep the whole sentence in one string.

`node i18n/scripts/i18n.mjs check` (part of `just check`) fails on invalid ICU, a missing note, a translation that breaks its placeholders, or one that says a fixed glossary word differently.

## Updating translations (before a release)

1. `node i18n/scripts/i18n.mjs prepare` writes `i18n/work/<lang>.json` for each language with new or changed English only, in the open tiers. The same English used in several places is one item (its other ids are in `also`; apply fills them all). Each item has the English, its note, the old translation when the English changed (`previous`), and a translation of the same sentence from another app when one exists (`suggestion`; reuse it unless the note calls for different wording).
2. Translate with subagents, a few languages each. Give every subagent `BRIEF.md`, `glossary/<lang>.json` and the work file, and nothing else about the codebase. It writes `i18n/work/<lang>.out.json` as `{ "<id>": "<translation>" }`.
   - If the work file has `glossaryTerms` (no glossary yet), first write `glossary/<lang>.json` mapping each term to the word that language's popular messengers use (Telegram's and Instagram's official translations when unsure).
3. `node i18n/scripts/i18n.mjs apply` checks each translation and writes the language files. Rejected items stay in the work file with `problems`; send only those back to the same subagent, then apply again. After two failed tries, leave the item: it shows in English. Apply also updates the iOS language list (`CFBundleLocalizations` in `mobile/ios/Runner/Info.plist`); commit that change with the language files.
4. Back-translation check: a different subagent translates the new strings back into English from the language files alone. Re-translate only the ones whose meaning changed.
5. `node i18n/scripts/i18n.mjs check`, then open one PR with the table from `node i18n/scripts/i18n.mjs status` (per language and tier: translated / total).

## Tiers

Text ships in rounds so every language stays at the same point: tier 1 is every short label (3 words or fewer) plus the screens people use daily, tier 2 the rest of everyday screens, tier 3 agents, projects and workflows. Untranslated text shows in English. To open the next round, raise `current` in `tiers.json` and run steps 1–5. If rules change, `node i18n/scripts/i18n.mjs park` moves translations above the current tier to `later/`; prepare offers them again as suggestions when their tier opens.

People may fix any translation directly in a language file. The script only re-asks a string when its English changes, so manual fixes stay.

## Adding a language

Add one entry to `languages.json` (`script` lists the Unicode scripts its letters use, empty for Latin), run steps 1–5, and the apps pick it up. If a language turns out broken, set `"enabled": false`: it disappears from the picker and its users see English.

## Pseudo language

`node i18n/scripts/i18n.mjs pseudo` writes `en-XA`: every string accented and about 40% longer. Run the app in it to spot text that was never moved into a language file and labels that overflow. These files are not committed.
