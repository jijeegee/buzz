import {
  type AgentNameClaim,
  type AgentNameRoster,
  agentNameTakenMessage,
  findAgentNameConflict,
} from "@/shared/api/agentNameConflict";
import { withoutSignedOutDevices } from "@/shared/api/ownerDevices";
import { relayClient } from "@/shared/api/relayClient";
import {
  agentNameKey,
  buildRemoteOwnedAgents,
  type RemoteOwnedAgent,
  remoteAgentPersonaId,
} from "@/shared/api/remoteOwnedAgents";
import { invokeTauri } from "@/shared/api/tauri";
import { getIdentity } from "@/shared/api/tauriIdentity";
import { listArchivedIdentities } from "@/shared/api/tauriIdentityArchive";
import { listPersonas } from "@/shared/api/tauriPersonas";
import { listAuthDevices } from "@/shared/api/tokenAuth";
import { fetchOwnerDevices } from "@/shared/api/useOwnerDevices";
import { KIND_MANAGED_AGENT } from "@/shared/constants/kinds";
import { normalizePubkey } from "@/shared/lib/pubkey";

type LocalAgentName = {
  pubkey: string;
  name: string;
  persona_id?: string | null;
};

/** An unreachable relay must not hold up creating an agent. */
const REMOTE_ROSTER_TIMEOUT_MS = 4_000;

function withTimeout<T>(promise: Promise<T>, ms: number): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error("timed out reading remote agents")),
      ms,
    );
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}

/**
 * The owner's agents running on their other computers, read from the relay
 * (see `remoteOwnedAgents.ts`). `localPubkeys` are left out.
 */
export function fetchRemoteOwnedAgents(
  ownerPubkey: string,
  localPubkeys: readonly string[],
): Promise<RemoteOwnedAgent[]> {
  return withTimeout(
    readRemoteOwnedAgents(ownerPubkey, localPubkeys),
    REMOTE_ROSTER_TIMEOUT_MS,
  );
}

async function readRemoteOwnedAgents(
  ownerPubkey: string,
  localPubkeys: readonly string[],
): Promise<RemoteOwnedAgent[]> {
  const owner = normalizePubkey(ownerPubkey);
  const [ownerDevices, devices] = await Promise.all([
    fetchOwnerDevices(owner),
    listAuthDevices().catch(() => []),
  ]);
  const live = withoutSignedOutDevices(
    ownerDevices,
    devices.map((device) => device.id),
  );
  const local = new Set(localPubkeys.map(normalizePubkey));
  const candidates = [...live.hostDevices.keys()].filter(
    (pubkey) => !local.has(pubkey),
  );
  const projections =
    candidates.length === 0
      ? []
      : await relayClient.fetchEvents({
          kinds: [KIND_MANAGED_AGENT],
          authors: [owner],
          "#d": candidates,
          limit: candidates.length * 4,
        });
  return buildRemoteOwnedAgents({
    hostDevices: live.hostDevices,
    currentDeviceIds: devices
      .filter((device) => device.current)
      .map((device) => device.id),
    localPubkeys: [...local],
    projections,
    ownerPubkey: owner,
  });
}

/**
 * Every agent and persona name the owner already uses: this computer's
 * records plus the agents on their other computers. A relay or archive read
 * that fails leaves those out rather than blocking the save.
 */
async function loadAgentNameRoster(): Promise<AgentNameRoster> {
  const [agents, personas] = await Promise.all([
    // Raw command: `listManagedAgents` lives in `tauri.ts`, which imports
    // this module lazily.
    invokeTauri<LocalAgentName[]>("list_managed_agents"),
    listPersonas(),
  ]);
  const [archived, remote] = await Promise.all([
    listArchivedIdentities()
      .then((snapshot) => new Set(snapshot.archived.map(normalizePubkey)))
      .catch(() => new Set<string>()),
    getIdentity()
      .then((identity) =>
        fetchRemoteOwnedAgents(
          identity.pubkey,
          agents.map((agent) => agent.pubkey),
        ),
      )
      .catch(() => [] as RemoteOwnedAgent[]),
  ]);
  return {
    agents: [
      ...agents
        .filter((agent) => !archived.has(normalizePubkey(agent.pubkey)))
        .map((agent) => ({
          pubkey: agent.pubkey,
          name: agent.name,
          personaId: agent.persona_id ?? null,
        })),
      ...remote
        .filter((agent) => agent.name !== null)
        .map((agent) => ({
          pubkey: agent.pubkey,
          name: agent.name ?? "",
          personaId: remoteAgentPersonaId(agent, personas),
        })),
    ],
    personas,
  };
}

/**
 * Throws when `claim.name` is already one of the owner's agent names. A
 * rename to the name it already has is always allowed (existing duplicates
 * stay editable), and a roster that cannot be read does not block the save.
 */
export async function assertAgentNameAvailable(
  claim: AgentNameClaim,
): Promise<void> {
  let roster: AgentNameRoster;
  try {
    roster = await loadAgentNameRoster();
  } catch {
    return;
  }
  const key = agentNameKey(claim.name);
  if (claim.kind === "agent" && claim.selfPubkey) {
    const selfPubkey = claim.selfPubkey.toLowerCase();
    const self = roster.agents.find(
      (agent) => agent.pubkey.toLowerCase() === selfPubkey,
    );
    if (self && agentNameKey(self.name) === key) return;
    claim = { ...claim, personaId: claim.personaId ?? self?.personaId };
  }
  if (claim.kind === "persona" && claim.selfId) {
    const self = roster.personas.find((persona) => persona.id === claim.selfId);
    if (self && agentNameKey(self.displayName) === key) return;
  }
  const taken = findAgentNameConflict(claim, roster);
  if (taken !== null) throw new Error(agentNameTakenMessage(taken));
}
