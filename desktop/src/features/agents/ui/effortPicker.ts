import type {
  AcpConfigOptionValue,
  ManagedAgentBackend,
} from "@/shared/api/types";
import type { PersonaDropdownOption } from "./agentConfigOptions";

/**
 * Sentinel dropdown value for "no explicit effort" — reverts the agent to the
 * adapter default at the next spawn. Distinct from any adapter option value.
 */
export const EFFORT_DEFAULT_DROPDOWN_VALUE = "__effort_default__";

/**
 * Helper copy shown under the control when its options come from the runtime
 * catalog's pre-discovery fallback rather than the running session: the
 * fallback is the subset every current model accepts, but the adapter's real
 * list is model-dependent and only known once a session has advertised it.
 */
export const EFFORT_FALLBACK_HELPER_TEXT = "Options may vary by model.";

/**
 * Pure gating + option compute for the effort write control.
 *
 * The picker is a LOCAL-only, Save-gated write control: the create dialog
 * embeds the selection in `create_managed_agent` and the edit dialog in the
 * locked `update_managed_agent` payload (PR #4625); the Rust backend rejects
 * both for non-local backends (remote effort is set at deploy time via
 * `policy_env`). So the UI must not offer it for a provider backend.
 *
 * Two option sources, in priority order:
 *   1. the running session's discovered `thought_level` option
 *      (`effortConfigId` + `effortOptions`, absent pre-first-session and for
 *      runtimes/models without effort support);
 *   2. the prospective runtime's catalog `effortThoughtLevel.fallbackValues` —
 *      the safe common subset the Rust catalog publishes for ACP thought-level
 *      harnesses (Claude Code, Codex, Hermes), so an effort can be picked
 *      before the first session, after a restart, or right after a runtime
 *      switch.
 *
 * `visible` is the single gate the dialogs render on: local backend AND
 * (a discovered `effortConfigId` OR a non-empty fallback list).
 */
export function effortPickerState({
  backend,
  effortConfigId,
  effortOptions,
  fallbackValues,
  currentEffort,
}: {
  backend: ManagedAgentBackend;
  effortConfigId: string | undefined;
  effortOptions: readonly AcpConfigOptionValue[] | undefined;
  /** `effortThoughtLevel.fallbackValues` of the prospective runtime's catalog entry. */
  fallbackValues: readonly string[] | null | undefined;
  currentEffort: string | null;
}): {
  visible: boolean;
  options: PersonaDropdownOption[];
  selectValue: string;
  /** True when the offered options came from the catalog fallback. */
  usingFallback: boolean;
} {
  const fallback = fallbackValues ?? [];
  const visible =
    backend.type === "local" &&
    (effortConfigId !== undefined || fallback.length > 0);

  const discovered = effortOptions ?? [];
  const usingFallback = discovered.length === 0 && fallback.length > 0;
  const values: AcpConfigOptionValue[] = usingFallback
    ? fallback.map((value) => ({
        value,
        displayName: labelForFallbackValue(value),
      }))
    : [...discovered];

  // Preselect the currently-configured effort when it maps to a known option;
  // otherwise fall back to the adapter-default sentinel (also the null case).
  // Fallback lists are a subset of what the model may accept, so a saved value
  // outside the list (e.g. `xhigh` picked from a past discovered session) is
  // kept selectable under its raw name rather than misreported as "default".
  const trimmed = currentEffort?.trim() ?? "";
  const known = trimmed.length > 0 && values.some((o) => o.value === trimmed);
  if (usingFallback && trimmed.length > 0 && !known) {
    values.push({ value: trimmed });
  }
  const selectValue =
    trimmed.length > 0 && (known || usingFallback)
      ? trimmed
      : EFFORT_DEFAULT_DROPDOWN_VALUE;

  const options: PersonaDropdownOption[] = [
    { label: "Adapter default", value: EFFORT_DEFAULT_DROPDOWN_VALUE },
    ...values.map((option) => ({
      label: option.displayName ?? option.value,
      value: option.value,
    })),
  ];

  return { visible, options, selectValue, usingFallback };
}

/** `low` → `Low`; fallback values are bare adapter ids with no display name. */
function labelForFallbackValue(value: string): string {
  return value.length > 0 ? value[0].toUpperCase() + value.slice(1) : value;
}

/**
 * Map a dropdown selection back to the persisted value sent as
 * `effortLevel` in the locked update payload: the sentinel clears effort
 * (null → adapter default), any other value is the explicit effort level.
 */
export function effortSelectionToPersistedValue(value: string): string | null {
  return value === EFFORT_DEFAULT_DROPDOWN_VALUE ? null : value;
}
