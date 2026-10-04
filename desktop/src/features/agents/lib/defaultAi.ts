import type { ManagedAgent } from "@/shared/api/types";

type DefaultAiCandidate = Pick<ManagedAgent, "pubkey" | "isDefaultAi">;

/**
 * Shown wherever a default-AI affordance has nothing to act on. Names the one
 * place the star is chosen (the routing agent picker in the Agents page
 * "Channel routing" card) so the empty state is a pointer, not a dead end.
 */
export const NO_DEFAULT_AI_HINT =
  "No routing agent yet — choose one in Agents › Channel routing";

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
 * `false` whenever no agent is starred (the row is shown disabled and off
 * then, but the stored preference may still be on).
 */
export function resolveAddDefaultAi(
  defaultAi: DefaultAiCandidate | null | undefined,
  requested: boolean,
): boolean {
  return Boolean(defaultAi) && requested;
}
