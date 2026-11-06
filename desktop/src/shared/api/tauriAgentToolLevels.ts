import { invokeTauri } from "@/shared/api/tauri";

/** Owner tools on (`full`) or off (`limited`). */
export type ToolLevel = "full" | "limited";

/** One agent's owner tools as the relay holds them. */
export type AgentToolLevel = {
  pubkey: string;
  /** `null` when never set, which acts as off. */
  level: ToolLevel | null;
  respondClass: string | null;
  /** Whether this computer runs the agent. */
  here: boolean;
};

/** Owner tools of `pubkeys` on the active account. A key missing from the
 * answer is unknown (shown off). */
export function getAgentToolLevels(
  pubkeys: string[],
): Promise<AgentToolLevel[]> {
  return invokeTauri<AgentToolLevel[]>("get_agent_tool_levels", { pubkeys });
}
