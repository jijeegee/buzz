import { useQueries } from "@tanstack/react-query";
import * as React from "react";

import {
  useManagedAgentsQuery,
  usePersonasQuery,
} from "@/features/agents/hooks";
import {
  buildRouterRoster,
  MAX_ROUTER_ROSTER,
  type RosterIdentity,
  routerHumanNames,
} from "@/features/messages/lib/routerRoster";
import type { RouterRosterEntry } from "@/shared/api/tauriMessageRouting";
import { getUserProfile } from "@/shared/api/tauriProfiles";
import { normalizePubkey } from "@/shared/lib/pubkey";

export type RouterRosterSnapshot = {
  roster: RouterRosterEntry[];
  humans: string[];
};

/**
 * Smart routing's roster inputs for one composer: the channel's agent
 * members (from the mention identities), my managed agents' personas, and
 * the kind:0 `about` of the others (same `user-profile` cache the profile
 * panel uses). Nothing is fetched unless `enabled` (Smart routing applied),
 * so other modes pay nothing. `getRoster` reads the latest inputs at call
 * time and is stable across renders.
 */
export function useRouterRosterSource({
  enabled,
  getIdentities,
  memberPubkeys,
  selfPubkey,
}: {
  enabled: boolean;
  getIdentities: () => readonly RosterIdentity[];
  memberPubkeys: ReadonlySet<string>;
  selfPubkey: string | null;
}): () => RouterRosterSnapshot {
  const managedAgents = useManagedAgentsQuery({ enabled }).data;
  const personas = usePersonasQuery({ enabled }).data;
  const identities = enabled ? getIdentities() : [];
  const managedPubkeys = new Set(
    (managedAgents ?? []).map((agent) => normalizePubkey(agent.pubkey)),
  );
  // Only agents whose capability text is not already local need a profile.
  const remoteAgentKey = identities
    .filter(
      (identity) =>
        identity.isAgent &&
        memberPubkeys.has(normalizePubkey(identity.pubkey)) &&
        !managedPubkeys.has(normalizePubkey(identity.pubkey)),
    )
    .map((identity) => normalizePubkey(identity.pubkey))
    .slice(0, MAX_ROUTER_ROSTER)
    .join(",");
  const remoteAgentPubkeys = React.useMemo(
    () => (remoteAgentKey ? remoteAgentKey.split(",") : []),
    [remoteAgentKey],
  );
  const profiles = useQueries({
    queries: remoteAgentPubkeys.map((pubkey) => ({
      queryKey: ["user-profile", pubkey],
      queryFn: () => getUserProfile(pubkey),
      staleTime: 60_000,
    })),
  });

  const latestRef = React.useRef({
    aboutByPubkey: new Map<string, string | null>(),
    getIdentities,
    managedAgents,
    memberPubkeys,
    personas,
    selfPubkey,
  });
  latestRef.current = {
    aboutByPubkey: new Map(
      remoteAgentPubkeys.map((pubkey, index) => [
        pubkey,
        profiles[index]?.data?.about ?? null,
      ]),
    ),
    getIdentities,
    managedAgents,
    memberPubkeys,
    personas,
    selfPubkey,
  };

  return React.useCallback(() => {
    const latest = latestRef.current;
    const identities = latest.getIdentities();
    return {
      roster: buildRouterRoster({
        aboutByPubkey: latest.aboutByPubkey,
        identities,
        managedAgents: latest.managedAgents ?? [],
        memberPubkeys: latest.memberPubkeys,
        personas: latest.personas ?? [],
        selfPubkey: latest.selfPubkey,
      }),
      humans: routerHumanNames(identities, latest.memberPubkeys),
    };
  }, []);
}
