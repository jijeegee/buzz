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
import { useOwnerDevices } from "@/shared/api/useOwnerDevices";
import { KIND_MANAGED_AGENT } from "@/shared/constants/kinds";
import { normalizePubkey } from "@/shared/lib/pubkey";

/** A remote agent with the persona it was made from on this computer. */
export type RemoteOwnedAgentHere = RemoteOwnedAgent & {
  /** This computer's persona id for it, or `null` (made without one). */
  localPersonaId: string | null;
};

/**
 * The signed-in user's agents running on their other computers (see
 * `remoteOwnedAgents.ts`). Follows the owner's device records live; the
 * agents' names come from their kind:30177 projections.
 */
export function useRemoteOwnedAgents(): RemoteOwnedAgentHere[] {
  const identity = useIdentityQuery();
  const owner = normalizePubkey(identity.data?.pubkey ?? "");
  const ownerDevices = useOwnerDevices(owner.length > 0);
  const devices = useAuthDevicesQuery(owner.length > 0);
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

  return React.useMemo(() => {
    // Until this computer's own records load, every agent would look remote.
    if (!owner || managedAgents.data === undefined) return [];
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
    ownerDevices.hostDevices,
    currentDeviceIds,
    localPubkeys,
    projections.data,
    personas.data,
  ]);
}
