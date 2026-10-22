import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";

import {
  deleteManagedAgentWithRules,
  type ManagedAgentActionResult,
} from "@/features/agents/lib/managedAgentControlActions";
import { invalidateChannelMembersRosters } from "@/features/channels/rosterFreshness";
import { removeChannelMember } from "@/shared/api/tauri";
import type { Channel, ManagedAgent, RelayAgent } from "@/shared/api/types";
import { getRelayAgentChannelIds } from "@/features/profile/ui/UserProfilePanelUtils";

type DeleteManagedAgentRulesContext = Omit<
  Parameters<typeof deleteManagedAgentWithRules>[0],
  "agent"
>;

type DeleteProfileManagedAgentContext = DeleteManagedAgentRulesContext & {
  removeAgentFromAllChannels: (pubkey: string) => Promise<void>;
};

type UseProfileAgentDeletionInput = {
  channels?: readonly Channel[];
  deleteManagedAgent: DeleteManagedAgentRulesContext["deleteManagedAgent"];
  getAvailability: DeleteManagedAgentRulesContext["getAvailability"];
  relayAgents?: readonly RelayAgent[];
};

export function useProfileAgentDeletion({
  channels,
  deleteManagedAgent,
  getAvailability,
  relayAgents,
}: UseProfileAgentDeletionInput) {
  const queryClient = useQueryClient();
  const removeAgentFromAllChannels = React.useCallback(
    async (agentPubkey: string) => {
      const normalizedPubkey = agentPubkey.toLowerCase();
      const channelIds = new Set(
        getRelayAgentChannelIds(relayAgents, agentPubkey),
      );
      for (const channel of channels ?? []) {
        if (
          channel.memberPubkeys.some(
            (memberPubkey) => memberPubkey.toLowerCase() === normalizedPubkey,
          )
        ) {
          channelIds.add(channel.id);
        }
      }
      if (channelIds.size === 0) return;
      await Promise.allSettled(
        [...channelIds].map((channelId) =>
          removeChannelMember(channelId, agentPubkey),
        ),
      );
      // Direct writes bypass the member mutations' invalidation; without
      // this, the deleted agent stays in cached rosters for the freshness
      // window.
      await invalidateChannelMembersRosters(queryClient, channelIds);
    },
    [channels, queryClient, relayAgents],
  );

  const deleteManagedAgentRecord = React.useCallback(
    (agentToDelete: ManagedAgent) =>
      deleteProfileManagedAgent(agentToDelete, {
        channels: channels ?? [],
        deleteManagedAgent,
        getAvailability,
        relayAgents: relayAgents ?? [],
        removeAgentFromAllChannels,
        skipRemoteDeleteConfirm: true,
      }),
    [
      channels,
      deleteManagedAgent,
      getAvailability,
      relayAgents,
      removeAgentFromAllChannels,
    ],
  );

  return {
    deleteManagedAgentRecord,
    removeAgentFromAllChannels,
  };
}

export async function deleteProfileManagedAgent(
  agent: ManagedAgent,
  context: DeleteProfileManagedAgentContext,
): Promise<ManagedAgentActionResult> {
  const { removeAgentFromAllChannels, ...deleteContext } = context;
  const result = await deleteManagedAgentWithRules({
    agent,
    ...deleteContext,
  });
  if (result.cancelled) return result;

  await removeAgentFromAllChannels(agent.pubkey);
  return result;
}
