import * as React from "react";

import { useChannelRoutingQuery } from "@/features/agents/channelRoutingHooks";
import { useManagedAgentsQuery } from "@/features/agents/hooks";
import {
  findDefaultAi,
  routingJoinsNewChannels,
} from "@/features/agents/lib/defaultAi";

/**
 * The managed agent starred as this desktop's default AI, read from the
 * shared managed-agents query. `defaultAi` is `null` both while the list is
 * still loading and when no agent is starred; `isLoading` tells them apart.
 * `routingMode` is the saved channel routing mode (undefined while loading);
 * `joinsNewChannels` says whether the create forms offer the join at all.
 */
export function useDefaultAi() {
  const managedAgentsQuery = useManagedAgentsQuery();
  const agents = managedAgentsQuery.data;
  const defaultAi = React.useMemo(
    () => (agents ? findDefaultAi(agents) : null),
    [agents],
  );
  const routingMode = useChannelRoutingQuery().data?.mode;
  return {
    defaultAi,
    isLoading: managedAgentsQuery.isPending,
    routingMode,
    joinsNewChannels: routingJoinsNewChannels(routingMode),
  };
}
