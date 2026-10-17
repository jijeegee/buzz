import * as React from "react";
import { useQuery } from "@tanstack/react-query";

import {
  useManagedAgentsQuery,
  usePersonasQuery,
} from "@/features/agents/hooks";
import { useIdentityQuery } from "@/shared/api/hooks";
import { relayClient } from "@/shared/api/relayClient";
import {
  buildRemoteOwnedAgents,
  type RemoteOwnedAgent,
  remoteAgentPersonaId,
} from "@/shared/api/remoteOwnedAgents";
import { useAuthDevicesQuery } from "@/shared/api/useAuthDeviceNames";
import {
  fetchOwnerDevices,
  ownerDevicesQueryKey,
  useOwnerDevices,
} from "@/shared/api/useOwnerDevices";
import { KIND_MANAGED_AGENT } from "@/shared/constants/kinds";
import { normalizePubkey } from "@/shared/lib/pubkey";

/** A remote agent with the persona it was made from on this computer. */
export type RemoteOwnedAgentHere = RemoteOwnedAgent & {
  /** This computer's persona id for it, or `null` (made without one). */
  localPersonaId: string | null;
};

export type RemoteOwnedAgentsState = {
  agents: RemoteOwnedAgentHere[];
  /**
   * Still reading which agents run elsewhere (device records, device list
   * or names). A persona may turn out to have its agent elsewhere, so its
   * Start waits. Settles on failure too.
   */
  settling: boolean;
};

/**
 * The signed-in user's agents running on their other computers (see
 * `remoteOwnedAgents.ts`). Follows the owner's device records live; the
 * agents' names come from their kind:30177 projections. Empty until the
 * account's device list is known, so signed-out computers never count.
 */
export function useRemoteOwnedAgents(): RemoteOwnedAgentsState {
  const identity = useIdentityQuery();
  const owner = normalizePubkey(identity.data?.pubkey ?? "");
  const ownerDevices = useOwnerDevices(owner.length > 0);
  const devices = useAuthDevicesQuery(owner.length > 0);
  // Same cached query as `useOwnerDevices`, read for its loading state.
  const ownerDevicesQuery = useQuery({
    enabled: owner.length > 0,
    queryKey: ownerDevicesQueryKey(owner),
    queryFn: () => fetchOwnerDevices(owner),
    staleTime: 5 * 60_000,
  });
  const managedAgents = useManagedAgentsQuery();
  const personas = usePersonasQuery();

  const localPubkeys = React.useMemo(
    () =>
      (managedAgents.data ?? []).map((agent) => normalizePubkey(agent.pubkey)),
    [managedAgents.data],
  );
  const currentDeviceIds = React.useMemo(
    () =>
      (devices.data ?? [])
        .filter((device) => device.current)
        .map((device) => device.id),
    [devices.data],
  );
  const candidates = React.useMemo(() => {
    const local = new Set(localPubkeys);
    return [...ownerDevices.hostDevices.keys()]
      .filter((pubkey) => !local.has(pubkey))
      .sort();
  }, [ownerDevices.hostDevices, localPubkeys]);

  const projections = useQuery({
    enabled: owner.length > 0 && candidates.length > 0,
    queryKey: ["remote-agent-projections", owner, candidates],
    queryFn: () =>
      relayClient.fetchEvents({
        kinds: [KIND_MANAGED_AGENT],
        authors: [owner],
        "#d": candidates,
        limit: candidates.length * 4,
      }),
    staleTime: 60_000,
  });

  const settling =
    owner.length > 0 &&
    (managedAgents.data === undefined ||
      ownerDevicesQuery.isPending ||
      devices.isPending ||
      (candidates.length > 0 && projections.isPending));

  const agents = React.useMemo(() => {
    // Until this computer's own records load, every agent would look remote.
    if (!owner || managedAgents.data === undefined) return [];
    // An empty or failed device list cannot drop signed-out computers.
    if (!devices.data || devices.data.length === 0) return [];
    const remote = buildRemoteOwnedAgents({
      hostDevices: ownerDevices.hostDevices,
      currentDeviceIds,
      localPubkeys,
      projections: projections.data ?? [],
      ownerPubkey: owner,
    });
    const personaList = personas.data ?? [];
    return remote.map((agent) => ({
      ...agent,
      localPersonaId: remoteAgentPersonaId(agent, personaList),
    }));
  }, [
    owner,
    managedAgents.data,
    devices.data,
    ownerDevices.hostDevices,
    currentDeviceIds,
    localPubkeys,
    projections.data,
    personas.data,
  ]);
  return React.useMemo(() => ({ agents, settling }), [agents, settling]);
}
