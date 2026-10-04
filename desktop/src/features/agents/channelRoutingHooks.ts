import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { managedAgentsQueryKey } from "@/features/agents/hooks";
import {
  type ChannelRoutingMode,
  type ChannelRoutingStatus,
  getChannelRouting,
  setChannelRouting,
} from "@/shared/api/tauriChannelRouting";
import { useAppFocused } from "@/shared/lib/useDocumentVisible";

/**
 * Nested under the managed-agents key on purpose: every path that invalidates
 * the agent list (start, stop, restart, `agents-data-changed`) also refreshes
 * the applied routing state, which is derived from those same processes.
 */
export const channelRoutingQueryKey = [
  ...managedAgentsQueryKey,
  "channel-routing",
] as const;

/** Poll cadence while something is running, matching the agent-list poll. */
const CHANNEL_ROUTING_POLL_MS = 5_000;

export function useChannelRoutingQuery() {
  const appFocused = useAppFocused();
  return useQuery({
    queryKey: channelRoutingQueryKey,
    queryFn: getChannelRouting,
    refetchInterval: (query) => {
      if (!appFocused) return false;
      const status = query.state.data as ChannelRoutingStatus | undefined;
      // Running processes can exit or restart with no event, which is what
      // moves the applied state; with nothing running there is nothing to
      // watch until an agent-list invalidation refreshes this key.
      return status?.agents.some((agent) => agent.running)
        ? CHANNEL_ROUTING_POLL_MS
        : false;
    },
    refetchOnWindowFocus: false,
  });
}

/** One user action = one `set_channel_routing` carrying mode and agent. */
export function useSetChannelRoutingMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      mode,
      agentPubkey,
    }: {
      mode: ChannelRoutingMode;
      agentPubkey: string | null;
    }) => setChannelRouting(mode, agentPubkey),
    onSuccess: (status) => {
      queryClient.setQueryData(channelRoutingQueryKey, status);
    },
    onSettled: async () => {
      // The star moves with the mode, so the agent list changes too.
      await queryClient.invalidateQueries({ queryKey: managedAgentsQueryKey });
    },
  });
}
