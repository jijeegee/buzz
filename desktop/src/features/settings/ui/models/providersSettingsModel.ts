/**
 * Pure view model for Settings › Models › Providers.
 *
 * The tab is a provider-centric view over the same `GlobalAgentConfig`
 * record Agent defaults edits: `preferred_runtime` + `provider` say which
 * provider and sign-in path new agents start on, and `env_vars` holds the
 * API keys. Nothing here ever chooses a `model` — that stays in Agent
 * defaults — and a harness that bills a provider through its CLI login is
 * recognised only through the Rust catalog fact
 * `AcpRuntimeCatalogEntry.subscriptionProvider`, never a runtime-id check.
 */
import { getDefaultPersonaRuntime } from "@/features/agents/lib/resolvePersonaRuntime";
import {
  getProviderApiKeyEnvVar,
  getProviderApiKeyLabel,
  PERSONA_LLM_PROVIDER_OPTIONS,
  resetConfigForHarnessChange,
} from "@/features/agents/ui/agentConfigOptions";
import type {
  AcpRuntimeCatalogEntry,
  GlobalAgentConfig,
  GlobalAgentConfigSaveResult,
} from "@/shared/api/types";

export type ProviderPath = "subscription" | "api-key";

/**
 * Providers the tab lists, in display order. `relay-mesh` (shared compute)
 * carries no credential or sign-in and stays an Agent defaults choice.
 */
const LISTED_PROVIDER_IDS: readonly string[] = [
  "anthropic",
  "openai",
  "openai-compat",
  "openrouter",
  "databricks",
  "databricks_v2",
];

export type ProviderRow = {
  providerId: string;
  label: string;
  /** Harness whose CLI login bills this provider, from the catalog fact; null when none. */
  subscriptionRuntime: AcpRuntimeCatalogEntry | null;
  /** Secret env var the API-key path reads; null for providers without a typed secret (Databricks: host + OAuth). */
  apiKeyEnvVar: string | null;
  apiKeyLabel: string | null;
  apiKeyIsSet: boolean;
  /**
   * An earlier listed provider whose key row already manages the same env
   * var (OpenAI-compatible rides on the OpenAI key), so this row renders a
   * note instead of a second field over the same state.
   */
  sharesApiKeyWith: string | null;
  /**
   * Harness the API-key path launches: an available provider-selection
   * runtime (one with a `providerEnvVar`), the global preferred one when it
   * qualifies, else the picker's own order. Null when none is installed.
   */
  apiKeyRuntime: AcpRuntimeCatalogEntry | null;
  /** Which of this provider's paths new agents currently default to, if any. */
  defaultPath: ProviderPath | null;
};

export type ProviderGroup = {
  primary: ProviderRow;
  /** Rows folded under `primary` because they share its API key. */
  companions: ProviderRow[];
};

export type DefaultOption = {
  /** `${providerId}:${path}` — stable DOM id and test id suffix. */
  id: string;
  providerId: string;
  path: ProviderPath;
  /** "Anthropic · Subscription" */
  label: string;
  /** What choosing it launches, e.g. "Claude Code sign-in". */
  detail: string;
  runtime: AcpRuntimeCatalogEntry | null;
  /** False when the path's harness is missing or not installed. */
  available: boolean;
  selected: boolean;
};

function isProviderSelectionRuntime(runtime: AcpRuntimeCatalogEntry) {
  return runtime.providerEnvVar !== null;
}

/**
 * Which path of `row` the global config currently defaults to.
 *
 * Subscription is a `preferred_runtime` fact alone: the login-billed harness
 * ignores `provider`. The API-key path needs `provider === providerId` *and*
 * a preferred runtime that honours it (a provider-selection harness, or none
 * set — the picker then falls back to one).
 */
function currentDefaultPath(
  row: Pick<ProviderRow, "providerId" | "subscriptionRuntime">,
  globalConfig: GlobalAgentConfig,
  catalog: readonly AcpRuntimeCatalogEntry[],
): ProviderPath | null {
  const preferred = globalConfig.preferred_runtime;
  if (row.subscriptionRuntime && preferred === row.subscriptionRuntime.id) {
    return "subscription";
  }
  if (globalConfig.provider !== row.providerId) return null;
  const preferredEntry = preferred
    ? (catalog.find((runtime) => runtime.id === preferred) ?? null)
    : null;
  if (preferredEntry && !isProviderSelectionRuntime(preferredEntry)) {
    return null;
  }
  return "api-key";
}

export function buildProviderRows(
  catalog: readonly AcpRuntimeCatalogEntry[],
  globalConfig: GlobalAgentConfig,
  hideProviderIds?: ReadonlySet<string>,
): ProviderRow[] {
  const apiKeyRuntime = getDefaultPersonaRuntime(
    catalog.filter(isProviderSelectionRuntime),
    globalConfig.preferred_runtime,
  );
  const rows: ProviderRow[] = [];
  for (const providerId of LISTED_PROVIDER_IDS) {
    if (hideProviderIds?.has(providerId)) continue;
    const option = PERSONA_LLM_PROVIDER_OPTIONS.find(
      (candidate) => candidate.id === providerId,
    );
    if (!option) continue;
    const apiKeyEnvVar = getProviderApiKeyEnvVar(providerId);
    const subscriptionRuntime =
      catalog.find((runtime) => runtime.subscriptionProvider === providerId) ??
      null;
    rows.push({
      providerId,
      label: option.label,
      subscriptionRuntime,
      apiKeyEnvVar,
      apiKeyLabel: getProviderApiKeyLabel(providerId),
      apiKeyIsSet:
        apiKeyEnvVar !== null &&
        (globalConfig.env_vars[apiKeyEnvVar] ?? "").length > 0,
      sharesApiKeyWith: apiKeyEnvVar
        ? (rows.find((earlier) => earlier.apiKeyEnvVar === apiKeyEnvVar)
            ?.providerId ?? null)
        : null,
      apiKeyRuntime,
      defaultPath: currentDefaultPath(
        { providerId, subscriptionRuntime },
        globalConfig,
        catalog,
      ),
    });
  }
  return rows;
}

/** Fold key-sharing rows under the row that owns the key field. */
export function groupProviderRows(
  rows: readonly ProviderRow[],
): ProviderGroup[] {
  const groups: ProviderGroup[] = [];
  for (const row of rows) {
    const owner = row.sharesApiKeyWith
      ? groups.find(
          (group) => group.primary.providerId === row.sharesApiKeyWith,
        )
      : undefined;
    if (owner) {
      owner.companions.push(row);
    } else {
      groups.push({ primary: row, companions: [] });
    }
  }
  return groups;
}

/**
 * The single "Default for new agents" choice list: every provider × path
 * that has a harness in the catalog. `GlobalAgentConfig` holds one
 * `preferred_runtime` / `provider`, so exactly one option is selected.
 */
export function defaultOptions(rows: readonly ProviderRow[]): DefaultOption[] {
  const options: DefaultOption[] = [];
  for (const row of rows) {
    if (row.subscriptionRuntime) {
      const runtime = row.subscriptionRuntime;
      const available = runtime.availability === "available";
      options.push({
        id: `${row.providerId}:subscription`,
        providerId: row.providerId,
        path: "subscription",
        label: `${row.label} · Subscription`,
        detail: available
          ? `${runtime.label} sign-in`
          : `${runtime.label} sign-in — not installed`,
        runtime,
        available,
        selected: row.defaultPath === "subscription",
      });
    }
    const credential = row.apiKeyEnvVar ?? "workspace sign-in";
    options.push({
      id: `${row.providerId}:api-key`,
      providerId: row.providerId,
      path: "api-key",
      label: `${row.label} · ${row.apiKeyEnvVar ? "API key" : "Workspace"}`,
      detail: row.apiKeyRuntime
        ? `${row.apiKeyRuntime.label} with ${credential}`
        : "Needs Buzz Agent or Goose installed",
      runtime: row.apiKeyRuntime,
      available: row.apiKeyRuntime !== null,
      selected: row.defaultPath === "api-key",
    });
  }
  return options;
}

/**
 * The config to persist when the user picks `path` for `row`. Returns null
 * when the path has no harness or nothing would change.
 *
 * Never sets a model. A harness switch goes through
 * `resetConfigForHarnessChange` exactly like the Agent defaults dialog
 * (model and effort cleared, provider kept only for provider-selection
 * harnesses); a provider switch on the same harness clears the model, whose
 * meaning belongs to the provider.
 */
export function nextGlobalConfigForDefault(
  globalConfig: GlobalAgentConfig,
  row: ProviderRow,
  path: ProviderPath,
): GlobalAgentConfig | null {
  const runtime =
    path === "subscription" ? row.subscriptionRuntime : row.apiKeyRuntime;
  if (!runtime) return null;
  const harnessChanged = globalConfig.preferred_runtime !== runtime.id;
  const base = harnessChanged
    ? resetConfigForHarnessChange(globalConfig, runtime.id)
    : globalConfig;
  if (path === "subscription") {
    return harnessChanged ? base : null;
  }
  if (base.provider === row.providerId) {
    return harnessChanged ? base : null;
  }
  return { ...base, provider: row.providerId, model: null };
}

/**
 * One API-key save = one `set_global_agent_config`. An empty value is the
 * removal: Rust's `strip_empty_env_vars` drops the key on write.
 */
export function nextGlobalConfigForApiKey(
  globalConfig: GlobalAgentConfig,
  envVar: string,
  value: string,
): GlobalAgentConfig {
  return {
    ...globalConfig,
    env_vars: { ...globalConfig.env_vars, [envVar]: value.trim() },
  };
}

/** Same words as Agent defaults: a save can restart running local agents. */
export function saveNoticeText(result: GlobalAgentConfigSaveResult): string {
  const restarted = result.restarted_count;
  const failed = result.failed_restart_count;
  const failedSuffix =
    failed > 0
      ? ` ${failed} agent${failed === 1 ? "" : "s"} couldn't restart — check the Agents page.`
      : "";
  if (restarted > 0) {
    return `Saved. Restarted ${restarted} agent${restarted === 1 ? "" : "s"}.${failedSuffix}`;
  }
  return failed > 0 ? `Saved.${failedSuffix}` : "Saved.";
}
