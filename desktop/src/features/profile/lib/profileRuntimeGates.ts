import { isDefaultAiEligible } from "@/features/agents/lib/defaultAi";
import type { ManagedAgent } from "@/shared/api/types";

/**
 * Whether the profile panel offers the Runtime tab at all. This is the real
 * reachability condition for everything inside it (Start on launch, the
 * Default AI row, harness log): only an owner viewing an agent gets the tab,
 * and only when there is runtime material to show.
 */
export function shouldShowRuntimeTab({
  canOpenAgentLogs,
  diagnosticsFieldCount,
  instanceCount,
  isBot,
  isOwner,
  managedAgent,
  runtimeFieldCount,
}: {
  canOpenAgentLogs: boolean;
  diagnosticsFieldCount: number;
  /** Live plus archived persona instances. */
  instanceCount: number;
  isBot: boolean;
  isOwner: boolean | undefined;
  managedAgent: ManagedAgent | undefined;
  runtimeFieldCount: number;
}): boolean {
  return (
    isOwner === true &&
    isBot &&
    (managedAgent !== undefined ||
      runtimeFieldCount > 0 ||
      instanceCount > 0 ||
      diagnosticsFieldCount > 0 ||
      canOpenAgentLogs)
  );
}

/**
 * The handler the Runtime tab's "Default AI" row receives, or `undefined`
 * (no row) when the viewer does not own a managed agent that can be starred.
 * Backend-agnostic on purpose: the star only marks a record. Key-less
 * definition records are excluded because Rust's `set_default_ai` rejects
 * them; the gate just hides the dead affordance.
 */
export function defaultAiToggleFor<Handler extends () => void>({
  handler,
  isOwner,
  managedAgent,
}: {
  handler: Handler;
  isOwner: boolean | undefined;
  managedAgent: Pick<ManagedAgent, "pubkey"> | undefined;
}): Handler | undefined {
  return isOwner === true &&
    managedAgent !== undefined &&
    isDefaultAiEligible(managedAgent)
    ? handler
    : undefined;
}
