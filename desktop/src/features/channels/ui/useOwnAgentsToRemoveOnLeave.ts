import * as React from "react";

import { useManagedAgentsQuery } from "@/features/agents/hooks";
import { useChannelMembersQuery } from "@/features/channels/hooks";
import { ownAgentsToRemoveOnLeave } from "@/features/channels/lib/leaveWithAgents";
import { useIdentityQuery } from "@/shared/api/hooks";
import { normalizePubkey } from "@/shared/lib/pubkey";

/**
 * My agents in a room I may leave, for the leave confirmation's "Leave and
 * remove my agents" choice. Empty while loading, in a room without my agents,
 * and when I am the room's only owner (that leave is refused).
 */
export function useOwnAgentsToRemoveOnLeave(
  channelId: string | null,
  enabled: boolean,
): string[] {
  const identityQuery = useIdentityQuery();
  const membersQuery = useChannelMembersQuery(channelId, enabled);
  const managedAgentsQuery = useManagedAgentsQuery({ enabled });
  const currentPubkey = identityQuery.data?.pubkey;
  return React.useMemo(
    () =>
      ownAgentsToRemoveOnLeave(
        membersQuery.data ?? [],
        new Set(
          (managedAgentsQuery.data ?? []).map((agent) =>
            normalizePubkey(agent.pubkey),
          ),
        ),
        currentPubkey,
      ),
    [currentPubkey, managedAgentsQuery.data, membersQuery.data],
  );
}
