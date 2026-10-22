/**
 * Curated one-line descriptions for harness catalog entries.
 *
 * Content policy (agreed in the BYOH UX thread): exactly ONE neutral,
 * vendor-sourced category sentence per entry — what kind of tool it is —
 * with provenance cited inline. No feature inventories, no marketing
 * superlatives, no volatile model/provider claims. Operational setup copy
 * (status, hints) stays generated from runtime state, never hand-authored
 * here. Research: ~/.kuzz/RESEARCH/BYOH_CATALOG_IA.md.
 */

import { type MessageKey, t } from "@/shared/i18n";

/** Message keys; translated in `harnessDescription`. */
const HARNESS_DESCRIPTIONS: Record<string, MessageKey> = {
  // Built-in runtimes.
  "kuzz-agent": "settings.harnessCatalog.description.kuzzAgent",
  // Source: https://code.claude.com/docs/en/overview — "Claude Code is an
  // agentic coding tool" that lives in the terminal.
  claude: "settings.harnessCatalog.description.claude",
  // Source: https://developers.openai.com/codex — "Codex is OpenAI's coding
  // agent".
  codex: "settings.harnessCatalog.description.codex",
  // Source: https://block.github.io/goose/ — "an open source, extensible AI
  // agent".
  goose: "settings.harnessCatalog.description.goose",

  // Bundled presets — sources per RESEARCH/BYOH_CATALOG_IA.md.
  // Source: https://cursor.com/docs/cli/acp
  cursor: "settings.harnessCatalog.description.cursor",
  // Source: https://github.com/can1357/oh-my-pi
  omp: "settings.harnessCatalog.description.omp",
  // Sources: https://pi.dev/docs/latest, https://github.com/salman1993/pi-acp
  pi: "settings.harnessCatalog.description.pi",
  // Source: https://build.x.ai (docs unavailable during research; kept
  // deliberately conservative).
  grok: "settings.harnessCatalog.description.grok",
  // Source: https://github.com/anomalyco/opencode
  opencode: "settings.harnessCatalog.description.opencode",
  // Sources: https://github.com/MoonshotAI/kimi-cli,
  // https://moonshotai.github.io/kimi-cli/en/
  kimi: "settings.harnessCatalog.description.kimi",
  // Sources: https://ampcode.com, https://ampcode.com/manual
  amp: "settings.harnessCatalog.description.amp",
  // Sources: https://github.com/NousResearch/hermes-agent,
  // https://hermes-agent.nousresearch.com/docs/
  hermes: "settings.harnessCatalog.description.hermes",
  // Sources: https://github.com/openclaw/openclaw,
  // https://docs.openclaw.ai/start/getting-started
  openclaw: "settings.harnessCatalog.description.openclaw",
};

/**
 * One neutral sentence describing the harness, or null for entries we don't
 * curate (customs, unknown ids). Callers must render nothing rather than
 * invent copy.
 */
export function harnessDescription(id: string): string | null {
  const key = HARNESS_DESCRIPTIONS[id.trim().toLowerCase()];
  return key ? t(key) : null;
}
