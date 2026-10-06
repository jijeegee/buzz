import { invokeTauri } from "@/shared/api/tauri";

/**
 * How agents pick up channel messages nobody @mentioned. One Rust-owned value
 * (`<app-data>/agents/channel-routing.json`); a missing file reads as `host`,
 * which is the behavior before the setting existed.
 */
export type ChannelRoutingMode = "off" | "host" | "lead" | "desktop-router";

/** The routing job one agent process has (or would get) at spawn. */
export type RoutingRole = "none" | "dispatcher" | "lead";

/** What is actually running right now — not what was saved. */
export type AppliedRouting =
  | { state: "off" }
  | { state: "hosting"; pubkey: string }
  | { state: "leading"; pubkey: string }
  | { state: "smart-routing" }
  | { state: "switching" };

/** One agent's place in the current mode transition (`plan_routing_transition`). */
export type AgentRoutingTransition = {
  pubkey: string;
  running: boolean;
  local: boolean;
  runningRole: RoutingRole;
  desiredRole: RoutingRole;
  /** Running with a role other than the one the saved mode gives it. */
  stale: boolean;
  /** Must not (re)start yet: another agent still runs a role it is losing. */
  hold: boolean;
};

export type ChannelRoutingStatus = {
  mode: ChannelRoutingMode;
  /** The starred routing agent, if any. */
  routingAgent: string | null;
  applied: AppliedRouting;
  routerActive: boolean;
  agents: AgentRoutingTransition[];
};

export async function getChannelRouting(): Promise<ChannelRoutingStatus> {
  return invokeTauri<ChannelRoutingStatus>("get_channel_routing");
}

/**
 * Save the mode and, for Host/Lead, the routing agent in one action. Rust
 * rejects a combination that is not ready (Host without an agent).
 */
export async function setChannelRouting(
  mode: ChannelRoutingMode,
  agentPubkey: string | null,
): Promise<ChannelRoutingStatus> {
  return invokeTauri<ChannelRoutingStatus>("set_channel_routing", {
    mode,
    agentPubkey,
  });
}
