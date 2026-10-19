/** Runtime provider-capability tri-state used by the submit path. */
export type ProviderRuntimeCapability = "capable" | "locked" | "unknown";

/**
 * Classify a runtime id's provider-selection capability as a tri-state,
 * independent of whether the runtime catalog has loaded yet.
 *
 * The submit path keys its provider write on this: "capable" persists the
 * provider, "locked" clears it, and "unknown" OMITS the field so a transient
 * loading/error state (or a genuinely unknown/custom command) never becomes a
 * destructive write.
 *
 * Before the catalog loads, `prospectiveRuntimeId` can already be a known
 * persona runtime string (e.g. `kuzz-agent`). A catalog lookup then returns
 * `undefined`, which would misclassify a provider-backed runtime as "unknown"
 * and omit the provider while still clearing the command override / writing env
 * — leaving the record inheriting a provider-backed runtime with a null
 * provider. To avoid that, we resolve capability STATICALLY for known ids:
 *
 * - kuzz-agent / goose → "capable" (`isProviderCapable`, id-based).
 * - claude / codex → "locked" (CLI-login runtimes; no LLM provider selection).
 * - anything else (custom, empty, genuinely unknown) → "unknown".
 *
 * `isProviderCapable` is the caller-supplied {@link
 * runtimeSupportsLlmProviderSelection} result, kept as the single source of
 * truth for the capable set rather than re-hardcoding it here.
 */
export function resolveRuntimeProviderCapability(
  runtimeId: string,
  isProviderCapable: boolean,
): ProviderRuntimeCapability {
  if (isProviderCapable) {
    return "capable";
  }
  const id = runtimeId.trim();
  if (id === "claude" || id === "codex") {
    return "locked";
  }
  return "unknown";
}

export function shouldClearModelForRuntimeChange(
  previousRuntime: string,
  nextRuntime: string,
): boolean {
  const previous = previousRuntime.trim();
  const next = nextRuntime.trim();

  return previous.length > 0 && previous !== next;
}

/**
 * Resolve the `agentCommand` field to send on Save.
 *
 * Every agent owns its command, so the command is sent only when it diverges
 * from the one the dialog opened with; `undefined` leaves it alone, so an
 * unchanged save stays a no-op. An empty custom command never reaches here
 * (the caller blocks Save for it; catalog runtimes always set a command).
 */
export function resolveAgentCommandUpdate(input: {
  /** The command currently in the (possibly prefilled) input. */
  agentCommand: string;
  /** The command the dialog opened with. */
  originalAgentCommand: string;
}): string | undefined {
  const command = input.agentCommand.trim();
  return command !== input.originalAgentCommand ? command : undefined;
}

/**
 * Decide whether Save should persist the effort picker's selection, and with
 * what value. Effort is embedded in the locked `update_managed_agent` payload
 * (PR #4625) — this runs only inside the submit path, so Cancel and a failed
 * Save never reach it, and no write is dispatched independently of the dialog
 * outcome.
 *
 * Persist only a real change: an unchanged selection is a no-op, so a
 * name-only edit never rewrites the effort column.
 */
export function resolveEffortSubmission(input: {
  effortLevel: string | null;
  originalEffortLevel: string | null;
}): { persist: boolean; level: string | null } {
  if (input.effortLevel === input.originalEffortLevel) {
    return { persist: false, level: null };
  }
  return { persist: true, level: input.effortLevel };
}

/**
 * Whether any of the runtime/provider-required credential keys is unset.
 *
 * A key counts as missing when its env value is absent or an empty string
 * (matching {@link EnvVarsEditor}'s own `isMissing` rendering). The
 * `requiredEnvKeys` list is already filtered to keys the dialog can fix —
 * CLI-login runtimes (claude/codex) and keys satisfied by the runtime file
 * config contribute no entries, so this never blocks on out-of-band auth.
 */
export function hasMissingRequiredEnvKey(
  requiredEnvKeys: readonly string[],
  envVars: Record<string, string>,
): boolean {
  return requiredEnvKeys.some((key) => (envVars[key] ?? "").length === 0);
}

/** Inputs for {@link computeEditAgentFormValidity} — all pre-derived primitives. */
export interface EditAgentFormValidityInput {
  name: string;
  parallelism: string;
  /** The ACP command already persisted on the agent. */
  agentAcpCommand: string;
  acpCommand: string;
  respondTo: string;
  respondToAllowlistLength: number;
  selectedRuntimeId: string;
  agentCommand: string;
  /**
   * Whether a runtime/provider-required credential key is still unset. When
   * true the Save button is blocked — the agent would otherwise persist with a
   * missing credential and crash-loop on next start. See
   * {@link hasMissingRequiredEnvKey}.
   */
  requiredEnvKeyMissing: boolean;
}

/**
 * Pure field-validity check for the Edit Agent dialog's Save button.
 *
 * Mirrors the harness/backend validation so the user sees a disabled button
 * instead of a round-tripped error:
 * - name is required;
 * - parallelism / timeout must be blank or parseable integers;
 * - a previously-set ACP command cannot be cleared to empty (spawn failure);
 * - allowlist respond-to mode needs at least one entry;
 * - a "Custom command" runtime must carry a concrete command — an empty
 *   command would spawn a runtime with no command.
 * - a runtime/provider-required credential key must be present — persisting
 *   with a missing key would crash-loop the agent on next start.
 */
export function computeEditAgentFormValidity(
  input: EditAgentFormValidityInput,
): boolean {
  const parallelismValid =
    input.parallelism.trim() === "" ||
    !Number.isNaN(Number.parseInt(input.parallelism, 10));
  const acpCommandValid = !(
    input.agentAcpCommand && input.acpCommand.trim() === ""
  );
  const respondToValid =
    input.respondTo !== "allowlist" || input.respondToAllowlistLength > 0;
  const customCommandValid = !(
    input.selectedRuntimeId === "custom" && input.agentCommand.trim() === ""
  );

  return (
    input.name.trim().length > 0 &&
    parallelismValid &&
    acpCommandValid &&
    respondToValid &&
    customCommandValid &&
    !input.requiredEnvKeyMissing
  );
}

export function isEditAgentProviderSaveValid({
  llmProviderFieldVisible,
  currentProvider,
  originalProvider,
  globalProvider,
  originalRuntimeSupportsProvider,
}: {
  llmProviderFieldVisible: boolean;
  currentProvider: string;
  originalProvider: string | null | undefined;
  globalProvider: string | null | undefined;
  originalRuntimeSupportsProvider: boolean;
}): boolean {
  if (!llmProviderFieldVisible) return true;
  if (currentProvider.trim() || (globalProvider ?? "").trim()) return true;
  return (
    (originalProvider ?? "").trim().length === 0 &&
    originalRuntimeSupportsProvider
  );
}

export function envVarsEqual(
  a: Record<string, string>,
  b: Record<string, string>,
): boolean {
  const aKeys = Object.keys(a);
  return (
    aKeys.length === Object.keys(b).length &&
    aKeys.every((key) => a[key] === b[key])
  );
}
