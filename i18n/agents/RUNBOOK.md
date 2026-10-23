# Translating with AI agents (runbook)

How a coding agent runs a translation round with sub-agents and no
translation API. Read `../TRANSLATING.md` first for the files, tiers and
checks. This page covers the rules and the order of steps, plus lessons
from the first round (Oct 2026).

## Rules

1. **English is the source.** Translators never edit `*.json`/`*.arb`
   language files directly; they write `i18n/work/**.out.json` and
   `i18n.mjs apply` validates and writes the files.
2. **Tiers decide what is translated.** `i18n/tiers.json` `current` is the
   highest open tier. Every language gets the same tiers at the same time
   (Korean is not done further than the others), so the owner can see how
   much is left with `node i18n/scripts/i18n.mjs status`.
   - Tier 1: every label of 3 words or fewer, server text, and the
     screens people use daily (chats, groups, friends, settings, ...).
   - Tier 2: the rest of the everyday screens (the default).
   - Tier 3: agent, project, workflow and goal screens.
   - Exception (owner-approved 2026-10-23): `languages` in `tiers.json`
     opens more tiers for single languages. ko/ja/zh-Hans/zh-Hant are at
     tier 3 because a full translation from before tiers could be reused
     (`prepare` puts it in `suggestion`; apply the suggestions as they are,
     then send only rejects to a fix agent).
   - `skip`: admin and developer screens and technical text. Never
     translated; it stays English.
3. **Product words never vary.** `i18n/glossary/terms.json` lists the
   product words (Chat, Group, Thread, Agent, Friend, ...). Words marked
   `fixed` must use the language's glossary form; `apply` rejects anything
   else. Change a glossary word only with the owner's agreement, and then fix
   every existing use (`check` lists them).
4. **Missing means English.** Untranslated text falls back to English in
   every app, so a partial language is safe to ship.
5. **One English text is translated once.** `prepare` emits one item per
   English text; `also` lists the other ids that get the same translation.
6. **Ask about cost first.** A round uses a noticeable share of the Claude
   weekly limit (numbers below). Tell the owner the estimate before starting,
   and report the actual usage after.

## Steps

```sh
R=<repo>; W=<scratch dir outside the repo>
cd $R
node i18n/scripts/i18n.mjs status                     # what is left
node i18n/scripts/i18n.mjs prepare es fr de ...        # work files for the open tiers
node i18n/agents/make_prompts.mjs $W es fr de ...      # chunks of 400, 2 chunks per prompt, $W/queue.txt
```

Then run translators from the queue, at most **20 sub-agents at a time** (the
harness limit):

```sh
node i18n/agents/pop.mjs $W/queue.txt 1      # next prompt path
```

Start each as a background general-purpose agent with the prompt
`Read and follow the instructions in <path> exactly. Reply only with the one-line-per-chunk result.`
When one finishes, start the next. If the harness says the concurrent limit is
reached, put the path back with `pop.mjs $W/queue.txt --back <path>`.

When all chunks of a language are done:

```sh
node i18n/agents/chunks.mjs status . <lang>   # chunks missing: none
node i18n/agents/chunks.mjs merge . <lang>    # -> i18n/work/<lang>.out.json
node i18n/scripts/i18n.mjs apply <lang>       # validates; rejects stay in i18n/work/<lang>.json
```

Fix rejects with one agent for several languages:

```sh
node i18n/agents/make_prompts.mjs $W --fix ru vi
# agent: "For each of ru, vi read and follow $W/fix_<lang>.md. Do not start sub-agents."
node i18n/scripts/i18n.mjs apply ru vi
```

A few rejects that are technical tokens (`party-parrot`, `NIP-05`, `URL`)
may stay English on purpose.

Finish:

```sh
node i18n/scripts/i18n.mjs check              # must end "language packs ok"
node --test i18n/scripts/i18n.test.mjs        # all pass
cd mobile && flutter gen-l10n && flutter analyze lib/l10n
node i18n/scripts/i18n.mjs status             # paste the table into the PR
```

Commit the language files, `i18n/state/`, `i18n/later/`, glossary changes and
`mobile/ios/Runner/Info.plist` (apply keeps its language list in step).
Never commit `i18n/work/`.

## Lessons from the first round

- **Usage.** Tier 1 for one language (4,233 texts, ~98k English characters)
  cost about 0.55% of the weekly Claude limit, and 9 languages took about
  32% of a 5-hour session. Tier 2 is about 22% of tier 1's text and tier 3
  about 60%, so estimate by characters (`status` gives counts). Measure
  tokens from transcripts and ask the owner for the weekly % before and
  after.
- **Stop early when the budget is tight.** When the owner asks to stop, let
  running agents finish, start no new languages, and keep the remaining
  prompts for later.
- **Sub-agents must not start sub-agents.** Every prompt ends with
  "Do not start sub-agents"; one translator did it once and doubled cost.
- **Reuse before translating.** `park` moves translations of tiers that are
  not open yet into `i18n/later/<lang>.json`; when the tier opens, `prepare`
  offers them as `suggestion`, so re-opening costs little.
- **Flutter needs base files.** gen-l10n refuses `zh_Hans`/`pt_BR` without
  a plain `app_zh.arb`/`app_pt.arb`. `apply` writes those copies. The mobile
  app hides them from users (`shippedLocales`).
- **Order of languages.** Finish the open tiers of the languages already
  shipped before adding new ones; add new languages in the order their
  regions open. Right-to-left languages (ar, fa, ur) also need the RTL
  layout in both apps.
- **Report in plain words**: which languages are done, what is left, and the
  usage, at least every 90 minutes during a long round.
