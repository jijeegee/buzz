import { normalizeDeviceId } from "@/shared/api/ownerDevices";
import type { RelayEvent } from "@/shared/api/types";
import { KIND_MANAGED_AGENT } from "@/shared/constants/kinds";

/**
 * The signed-in user's agents that run on another of their computers.
 *
 * Every computer lists the agents it hosts (kind:30180, `ownerDevices.ts`)
 * and publishes each agent's projection (kind:30177, `d` = agent pubkey,
 * content carries `name` and `persona_id`). Personas sync to every computer,
 * but the agent itself stays where it was made: this computer has no key for
 * it and must show it as that computer's agent, never as a startable copy.
 */
export type RemoteOwnedAgent = {
  /** Agent pubkey, lowercase hex. */
  pubkey: string;
  /** Relay device id of the computer hosting it, lowercase. */
  deviceId: string;
  /** Agent name from its projection; `null` until the projection is known. */
  name: string | null;
  /** Persona id on the hosting computer (its local id, not this one's). */
  personaId: string | null;
};

type ManagedAgentProjection = {
  pubkey: string;
  name: string | null;
  personaId: string | null;
  createdAt: number;
  id: string;
};

function parseProjection(
  event: RelayEvent,
  owner: string,
): ManagedAgentProjection | null {
  if (event.kind !== KIND_MANAGED_AGENT) return null;
  if (event.pubkey.toLowerCase() !== owner) return null;
  const pubkey = event.tags
    .find((tag) => tag[0] === "d")?.[1]
    ?.trim()
    .toLowerCase();
  if (!pubkey) return null;
  let content: unknown;
  try {
    content = JSON.parse(event.content);
  } catch {
    return null;
  }
  if (typeof content !== "object" || content === null) return null;
  const { name, persona_id: personaId } = content as {
    name?: unknown;
    persona_id?: unknown;
  };
  return {
    pubkey,
    name: typeof name === "string" && name.trim() ? name.trim() : null,
    personaId:
      typeof personaId === "string" && personaId.trim()
        ? personaId.trim()
        : null,
    createdAt: event.created_at,
    id: event.id,
  };
}

/**
 * Remote agents from the owner's host-device map: agents hosted on a device
 * other than this one and with no local record here, named from the newest
 * projection the owner published for each.
 */
export function buildRemoteOwnedAgents(input: {
  /** Agent pubkey → hosting device id (signed-out devices already dropped). */
  hostDevices: ReadonlyMap<string, string>;
  /** Device ids of this computer (the account's `current` device). */
  currentDeviceIds: readonly string[];
  /** Pubkeys of the agents this computer has records for. */
  localPubkeys: readonly string[];
  projections: readonly RelayEvent[];
  ownerPubkey: string;
}): RemoteOwnedAgent[] {
  const owner = input.ownerPubkey.trim().toLowerCase();
  const current = new Set(input.currentDeviceIds.map(normalizeDeviceId));
  const local = new Set(input.localPubkeys.map((key) => key.toLowerCase()));
  const newest = new Map<string, ManagedAgentProjection>();
  for (const event of input.projections) {
    const projection = parseProjection(event, owner);
    if (!projection) continue;
    const previous = newest.get(projection.pubkey);
    if (
      !previous ||
      projection.createdAt > previous.createdAt ||
      (projection.createdAt === previous.createdAt &&
        projection.id < previous.id)
    ) {
      newest.set(projection.pubkey, projection);
    }
  }
  const agents: RemoteOwnedAgent[] = [];
  for (const [pubkey, deviceId] of input.hostDevices) {
    if (local.has(pubkey) || current.has(deviceId)) continue;
    const projection = newest.get(pubkey);
    agents.push({
      pubkey,
      deviceId,
      name: projection?.name ?? null,
      personaId: projection?.personaId ?? null,
    });
  }
  return agents.sort((a, b) => a.pubkey.localeCompare(b.pubkey));
}

/** Names compare trimmed and case-insensitively. */
export function agentNameKey(name: string | null | undefined): string {
  return (name ?? "").trim().toLocaleLowerCase();
}

/**
 * This computer's persona a remote agent was made from. The hosting computer
 * publishes its own persona id; a persona it created in-app keeps that id
 * everywhere, but one installed from a team has a different local id on
 * each computer, so an unmatched id falls back to the persona's name (agent
 * names are unique per owner). An agent made without a persona matches one
 * of the same name the same way: both would be one name to the owner.
 */
export function remoteAgentPersonaId(
  agent: Pick<RemoteOwnedAgent, "name" | "personaId">,
  personas: readonly { id: string; displayName: string }[],
): string | null {
  if (agent.personaId) {
    const byId = personas.find((persona) => persona.id === agent.personaId);
    if (byId) return byId.id;
  }
  const key = agentNameKey(agent.name);
  if (!key) return null;
  const byName = personas.filter(
    (persona) => agentNameKey(persona.displayName) === key,
  );
  return byName.length === 1 ? byName[0].id : null;
}
