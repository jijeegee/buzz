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
    /**
     * Runs on another computer, made from a persona this computer does not
     * have (yet). A persona added here under that name is taken to be it.
     */
    personaElsewhere?: boolean;
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
  // Renaming a persona renames only its agents that carry its current name;
  // one with its own name (e.g. "Fizz 2") keeps it and still holds it.
  const renamedPersonaName =
    claim.kind === "persona" && ownPersonaId
      ? roster.personas.find((persona) => persona.id === ownPersonaId)
          ?.displayName
      : undefined;
  for (const agent of roster.agents) {
    if (agentNameKey(agent.name) !== key) continue;
    if (selfPubkey && agent.pubkey.toLowerCase() === selfPubkey) continue;
    // Renaming a persona keeps its own agents' names company.
    if (
      claim.kind === "persona" &&
      ownPersonaId &&
      agent.personaId === ownPersonaId &&
      (renamedPersonaName === undefined || agent.name === renamedPersonaName)
    ) {
      continue;
    }
    if (claim.kind === "persona" && agent.personaElsewhere) continue;
    return agent.name;
  }
  for (const persona of roster.personas) {
    if (agentNameKey(persona.displayName) !== key) continue;
    if (ownPersonaId && persona.id === ownPersonaId) continue;
    return persona.displayName;
  }
  return null;
}

/**
 * `name` when it is free, else the first free "name 2", "name 3", … (for a
 * deliberate extra agent of a persona, which may not reuse its name).
 */
export function nextFreeAgentName(
  claim: Extract<AgentNameClaim, { kind: "agent" }>,
  roster: AgentNameRoster,
): string {
  const base = claim.name.trim();
  if (findAgentNameConflict(claim, roster) === null) return base;
  for (let suffix = 2; ; suffix++) {
    const name = `${base} ${suffix}`;
    // A numbered name is never the persona's own, so it shares with nothing.
    if (findAgentNameConflict({ ...claim, name, personaId: null }, roster)) {
      continue;
    }
    return name;
  }
}

export function agentNameTakenMessage(name: string): string {
  return `You already have an agent named "${name.trim()}". Choose a different name.`;
}
