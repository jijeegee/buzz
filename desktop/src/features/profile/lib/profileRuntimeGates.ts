import type { ManagedAgent } from "@/shared/api/types";

/**
 * Whether the profile panel offers the Runtime tab at all. This is the real
 * reachability condition for everything inside it (Start on launch,
 * harness log): only an owner viewing an agent gets the tab,
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
