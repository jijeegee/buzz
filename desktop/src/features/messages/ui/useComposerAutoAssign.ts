import { useQuery, useQueryClient } from "@tanstack/react-query";
import * as React from "react";

import {
  useRoutingAgentApplied,
  useSmartRoutingActive,
} from "@/features/agents/channelRoutingHooks";
import { isAgentMentionChannelType } from "@/features/agents/lib/agentAutocompleteEligibility";
import { holdAutoRouteBatch } from "@/features/messages/lib/autoRouteBatcher";
import type { UseMentionsResult } from "@/features/messages/lib/useMentions";
import { channelMessagesKey } from "@/features/messages/lib/messageQueryKeys";
import { selectRouterRecent } from "@/features/messages/lib/routerRecent";
import { taskModelsQueryKey } from "@/features/settings/taskModelsHooks";
import {
  getTaskModels,
  MESSAGE_ROUTING_TASK_ID,
} from "@/shared/api/tauriMessageRouting";
import type { RelayEvent } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { useAutoAssign } from "./useAutoAssign";
import { useRouterRosterSource } from "./useRouterRosterSource";

/**
 * Wires Smart routing into one `MessageComposer`: the applied mode and model
 * readiness, the channel roster, and the after-send `useAutoAssign` routing.
 * Everything stays idle (no queries, no timers) unless Smart routing is the
 * applied channel routing. While this composer holds a draft, its
 * channel's routing batch waits (`autoRouteBatcher`).
 */
export function useComposerAutoAssign({
  addressedAgentCount,
  channelId,
  channelType,
  hasDraft,
  isEditing,
  mentions,
  selfPubkey,
  threadRoot,
}: {
  addressedAgentCount: number;
  channelId: string | null;
  channelType: string | null | undefined;
  /** The composer holds unsent text. */
  hasDraft: boolean;
  isEditing: boolean;
  mentions: UseMentionsResult;
  selfPubkey: string | null;
  threadRoot: string | null;
}) {
  const routerActive = useSmartRoutingActive();
  const routerModel = useQuery({
    enabled: routerActive,
    queryKey: taskModelsQueryKey,
    queryFn: getTaskModels,
    select: (tasks) =>
      tasks.find(
        (task) => task.taskId === MESSAGE_ROUTING_TASK_ID && task.ready,
      ),
    staleTime: 30_000,
  }).data;
  const routerReady = routerModel !== undefined;
  const getRoster = useRouterRosterSource({
    enabled: routerActive,
    getIdentities: mentions.getMentionIdentities,
    memberPubkeys: mentions.memberPubkeys,
    selfPubkey,
  });
  const queryClient = useQueryClient();
  const getRecent = React.useCallback(
    (message: RelayEvent, exclude: ReadonlySet<string>) => {
      const sentChannelId =
        message.tags.find((tag) => tag[0] === "h")?.[1] ?? channelId;
      if (!sentChannelId) return [];
      const cached =
        queryClient.getQueryData<RelayEvent[]>(
          channelMessagesKey(sentChannelId),
        ) ?? [];
      return selectRouterRecent(cached, message, selfPubkey, exclude);
    },
    [channelId, queryClient, selfPubkey],
  );
  const composerId = React.useId();
  const holding = routerActive && hasDraft && !isEditing;
  React.useEffect(() => {
    if (!holding || !channelId) return;
    holdAutoRouteBatch(channelId, composerId, true);
    return () => holdAutoRouteBatch(channelId, composerId, false);
  }, [channelId, composerId, holding]);
  // Host/Lead: the routing agent reads every message in channels it belongs
  // to, so there an agent mention needs no `p` tag to be seen.
  const routingAgent = useRoutingAgentApplied();
  const routedByAgent =
    routingAgent !== null &&
    !isEditing &&
    isAgentMentionChannelType(channelType) &&
    mentions.memberPubkeys.has(normalizePubkey(routingAgent));
  const { routeAfterSend } = useAutoAssign({
    addressedAgentCount,
    channelId,
    getRecent,
    channelType,
    getRoster,
    isEditing,
    routerActive,
    routerReady,
    threadRoot,
  });
  return { routeAfterSend, routedByAgent };
}
