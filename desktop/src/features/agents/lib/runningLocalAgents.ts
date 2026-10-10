import {
  listManagedAgentRuntimes,
  stopManagedAgentRuntime,
} from "@/shared/api/tauriManagedAgents";
import type { ManagedAgentRuntimeStatus } from "@/shared/api/types";

/** Local agent processes running on this computer, by agent and by pair. */
export type RunningLocalAgents = {
  /** Distinct agent pubkeys with at least one live runtime. */
  pubkeys: string[];
  /** Every live (agent, community) runtime, as the native stop command takes. */
  pairs: { pubkey: string; relayUrl: string }[];
};

const NOT_RUNNING = new Set(["stopped", "failed"]);

export function runningLocalAgentsFrom(
  runtimes: readonly ManagedAgentRuntimeStatus[],
): RunningLocalAgents {
  const pairs = runtimes
    .filter((runtime) => !NOT_RUNNING.has(runtime.lifecycle))
    .map(({ pubkey, relayUrl }) => ({ pubkey, relayUrl }));
  return { pubkeys: [...new Set(pairs.map((pair) => pair.pubkey))], pairs };
}

/** Running local agents; none when the list cannot be read. */
export async function listRunningLocalAgents(): Promise<RunningLocalAgents> {
  try {
    return runningLocalAgentsFrom(await listManagedAgentRuntimes());
  } catch (error) {
    console.error("[agents] could not list running agents:", error);
    return { pubkeys: [], pairs: [] };
  }
}

/**
 * Stop every listed runtime through the existing per-pair stop. Configs are
 * kept. Resolves with the number of runtimes that could not be stopped.
 */
export async function stopRunningLocalAgents(
  running: RunningLocalAgents,
): Promise<number> {
  const results = await Promise.allSettled(
    running.pairs.map(({ pubkey, relayUrl }) =>
      stopManagedAgentRuntime(pubkey, relayUrl),
    ),
  );
  return results.filter((result) => result.status === "rejected").length;
}

/** "1 agent is" / "N agents are". */
export function agentCountPhrase(count: number): string {
  return count === 1 ? "1 agent is" : `${count} agents are`;
}
