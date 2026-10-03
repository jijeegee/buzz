import type { ManagedAgent } from "@/shared/api/types";

type DefaultAiCandidate = Pick<ManagedAgent, "pubkey" | "isDefaultAi">;

/**
 * A key-less definition record has no identity to star. This mirrors the
 * gate in Rust's `set_default_ai` only so the UI can hide the affordance;
 * Rust remains the enforcer and rejects such a request regardless.
 */
export function isDefaultAiEligible(
  agent: Pick<ManagedAgent, "pubkey">,
): boolean {
  return agent.pubkey.trim().length > 0;
}

/**
 * The agent currently starred as the default AI, or `null` when none is.
 * Rust guarantees at most one record carries the flag; an ineligible record
 * that somehow carries it is ignored rather than surfaced.
 */
export function findDefaultAi<T extends DefaultAiCandidate>(
  agents: readonly T[],
): T | null {
  return (
    agents.find((agent) => agent.isDefaultAi && isDefaultAiEligible(agent)) ??
    null
  );
}

/**
 * What a create form submits as `addDefaultAi`: the switch value, forced to
 * `false` whenever no agent is starred (the switch is hidden then, but the
 * stored preference may still be on).
 */
export function resolveAddDefaultAi(
  defaultAi: DefaultAiCandidate | null | undefined,
  requested: boolean,
): boolean {
  return Boolean(defaultAi) && requested;
}

/**
 * The `set_default_managed_agent` argument for a star toggle: starring sends
 * the agent's pubkey, un-starring the current default clears the selection.
 */
export function defaultAiSelectionFor(
  pubkey: string,
  isDefaultAi: boolean,
): string | null {
  return isDefaultAi ? pubkey : null;
}

/**
 * The one-line confirmation the profile settings menu shows after a
 * successful default-AI toggle.
 */
export function defaultAiToggleNotice(
  name: string,
  isDefaultAi: boolean,
): string {
  return isDefaultAi
    ? `${name} is now your default AI.`
    : `${name} is no longer your default AI.`;
}
