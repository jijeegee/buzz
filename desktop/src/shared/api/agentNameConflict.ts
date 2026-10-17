import { agentNameKey } from "@/shared/api/remoteOwnedAgents";

/**
 * One owner never has two agents with the same name, on any of their
 * computers (akak 2026-10-17): names compare trimmed and case-insensitively.
 * A persona and the agents made from it share a name by design, so they do
 * not conflict with each other.
 */
export type AgentNameRoster = {
  agents: readonly {
    pubkey: string;
    name: string;
    /** Persona id on this computer, if the agent was made from one. */
    personaId: string | null;
  }[];
  personas: readonly { id: string; displayName: string }[];
};

export type AgentNameClaim =
  /** A new or renamed agent, made from `personaId` when set. */
  | {
      kind: "agent";
      name: string;
      personaId?: string | null;
      /** The agent being renamed, excluded from the roster. */
      selfPubkey?: string | null;
    }
  /** A new or renamed persona; `selfId` is set when renaming. */
  | { kind: "persona"; name: string; selfId?: string | null };

/** The name already taken by another agent or persona, or `null`. */
export function findAgentNameConflict(
  claim: AgentNameClaim,
  roster: AgentNameRoster,
): string | null {
  const key = agentNameKey(claim.name);
  if (!key) return null;
  const ownPersonaId =
    claim.kind === "agent" ? (claim.personaId ?? null) : (claim.selfId ?? null);
  const selfPubkey =
    claim.kind === "agent" ? (claim.selfPubkey ?? "").toLowerCase() : "";
  for (const agent of roster.agents) {
    if (agentNameKey(agent.name) !== key) continue;
    if (selfPubkey && agent.pubkey.toLowerCase() === selfPubkey) continue;
    // Renaming a persona keeps its own agents' names company.
    if (
      claim.kind === "persona" &&
      ownPersonaId &&
      agent.personaId === ownPersonaId
    ) {
      continue;
    }
    return agent.name;
  }
  for (const persona of roster.personas) {
    if (agentNameKey(persona.displayName) !== key) continue;
    if (ownPersonaId && persona.id === ownPersonaId) continue;
    return persona.displayName;
  }
  return null;
}

export function agentNameTakenMessage(name: string): string {
  return `You already have an agent named "${name.trim()}". Choose a different name.`;
}
