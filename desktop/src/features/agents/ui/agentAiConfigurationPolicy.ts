export type AgentAiConfigurationMode = "defaults" | "custom";

export type AgentAiConfigurationPair = {
  provider: string;
  model: string;
};

export function initialAgentAiConfigurationMode(
  pair: Partial<AgentAiConfigurationPair>,
): AgentAiConfigurationMode {
  return pair.provider?.trim() || pair.model?.trim() ? "custom" : "defaults";
}

export function agentAiConfigurationPairForMode({
  current,
  inherited,
  mode,
  needsProviderSelection = true,
}: {
  current: AgentAiConfigurationPair;
  inherited: AgentAiConfigurationPair;
  mode: AgentAiConfigurationMode;
  needsProviderSelection?: boolean;
}): AgentAiConfigurationPair {
  if (mode === "defaults") {
    return { provider: "", model: "" };
  }

  return {
    provider: needsProviderSelection
      ? current.provider.trim() || inherited.provider
      : "",
    model: current.model.trim() || inherited.model,
  };
}

/**
 * Whether a Customize (explicit) AI pair is complete enough to submit.
 *
 * `needsProviderSelection` reflects whether the provider picker is actually
 * shown to the user: Buzz Agent / Goose expose it (and runtime-less legacy /
 * builtin definitions do too), so both provider and model are required — the
 * backend readiness gate (`required_normalized_fields`) refuses to spawn those
 * harnesses without the pair. Every other harness (Codex / Claude / Hermes /
 * presets / custom commands) drives its own provider AND its own default
 * model, so neither is required: an empty model means "the harness's own
 * default" and the spawn simply omits `BUZZ_ACP_MODEL`. Requiring a model
 * there forced users to pick an entry from the discovered list even when the
 * harness's native default (e.g. a subscription-gated model the static picker
 * cannot know about) was what they wanted. Callers should pass the
 * field-visibility capability (`runtimeCanChooseLlmProvider`), not the raw
 * runtime capability, so the gate never diverges from the visible picker. It
 * defaults to `true` so existing callers keep the provider+model requirement.
 */
export function agentAiConfigurationSubmitBlockReason(
  mode: AgentAiConfigurationMode,
  pair: AgentAiConfigurationPair,
  needsProviderSelection = true,
): string | null {
  if (
    mode !== "custom" ||
    agentAiConfigurationModeSatisfied(mode, pair, needsProviderSelection)
  )
    return null;
  return !pair.provider.trim()
    ? "Choose a provider to save custom AI configuration."
    : "Choose a model to save custom AI configuration.";
}

export function agentAiConfigurationModeSatisfied(
  mode: AgentAiConfigurationMode,
  pair: AgentAiConfigurationPair,
  needsProviderSelection = true,
) {
  if (mode === "defaults" || !needsProviderSelection) {
    return true;
  }
  return pair.provider.trim().length > 0 && pair.model.trim().length > 0;
}
