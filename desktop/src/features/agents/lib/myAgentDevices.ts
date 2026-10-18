import type { ManagedAgent } from "@/shared/api/types";

/** The agents of one of the owner's other computers. */
export type RemoteDeviceGroup<Agent> = {
  deviceId: string;
  /** The computer's name; empty when the device list does not know it. */
  name: string;
  agents: Agent[];
};

/**
 * This computer's real agents for the Agents list and page: every agent
 * record that is not archived, one row each (several agents made from the
 * same preset each show), sorted by name.
 */
export function localAgentsForList(
  agents: readonly ManagedAgent[],
  isArchived: (pubkey: string) => boolean,
): ManagedAgent[] {
  return agents
    .filter((agent) => !isArchived(agent.pubkey))
    .sort((left, right) =>
      left.name.localeCompare(right.name, undefined, { sensitivity: "base" }),
    );
}

/**
 * The owner's agents on their other computers, one group per computer,
 * groups sorted by computer name (unnamed last) and agents by `nameOf`.
 */
export function groupRemoteAgentsByDevice<
  Agent extends { deviceId: string; pubkey: string },
>(
  agents: readonly Agent[],
  deviceName: (deviceId: string) => string | undefined,
  nameOf: (agent: Agent) => string,
): RemoteDeviceGroup<Agent>[] {
  const byDevice = new Map<string, Agent[]>();
  for (const agent of agents) {
    const list = byDevice.get(agent.deviceId) ?? [];
    list.push(agent);
    byDevice.set(agent.deviceId, list);
  }
  const compare = (left: string, right: string) =>
    left.localeCompare(right, undefined, { sensitivity: "base" });
  return [...byDevice]
    .map(([deviceId, list]) => ({
      deviceId,
      name: deviceName(deviceId)?.trim() ?? "",
      agents: [...list].sort(
        (left, right) =>
          compare(nameOf(left), nameOf(right)) ||
          left.pubkey.localeCompare(right.pubkey),
      ),
    }))
    .sort((left, right) => {
      if (!left.name !== !right.name) return left.name ? -1 : 1;
      return (
        compare(left.name, right.name) ||
        left.deviceId.localeCompare(right.deviceId)
      );
    });
}

/** "used by 2 agents" / "not used yet" for a preset card. */
export function presetUsageLabel(agentCount: number): string {
  if (agentCount <= 0) return "not used yet";
  return agentCount === 1 ? "used by 1 agent" : `used by ${agentCount} agents`;
}
