import { listManagedAgentRuntimes } from "@/shared/api/tauriManagedAgents";
import { invokeTauri } from "@/shared/api/tauri";
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
 * Running local agents owned by the active account only. Several accounts can
 * be signed in on this computer; signing out of one must never offer to stop
 * another account's agents.
 */
export async function listOwnRunningLocalAgents(): Promise<RunningLocalAgents> {
  const running = await listRunningLocalAgents();
  if (running.pubkeys.length === 0) return running;
  try {
    const own = new Set(
      await invokeTauri<string[]>("own_agent_pubkeys", {
        pubkeys: running.pubkeys,
      }),
    );
    return {
      pubkeys: running.pubkeys.filter((pubkey) => own.has(pubkey)),
      pairs: running.pairs.filter((pair) => own.has(pair.pubkey)),
    };
  } catch (error) {
    console.error("[agents] could not check agent owners:", error);
    return { pubkeys: [], pairs: [] };
  }
}
