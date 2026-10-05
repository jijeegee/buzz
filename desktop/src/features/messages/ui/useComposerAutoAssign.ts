import { useQuery } from "@tanstack/react-query";
import * as React from "react";

import { useSmartRoutingActive } from "@/features/agents/channelRoutingHooks";
import type { UseMentionsResult } from "@/features/messages/lib/useMentions";
import { taskModelsQueryKey } from "@/features/settings/taskModelsHooks";
import {
  getTaskModels,
  MESSAGE_ROUTING_TASK_ID,
} from "@/shared/api/tauriMessageRouting";
import { useAutoAssign } from "./useAutoAssign";
import { useRouterRosterSource } from "./useRouterRosterSource";

/**
 * Wires Smart routing into one `MessageComposer`: the applied mode and model
 * readiness, the channel roster, and the draft-scoped `useAutoAssign` state.
 * Everything stays idle (no queries, no timers) unless Smart routing is the
 * applied channel routing.
 */
export function useComposerAutoAssign({
  addressedAgentCount,
  channelId,
  channelType,
  draftKey,
  isEditing,
  mentions,
  selfPubkey,
  threadRoot,
}: {
  addressedAgentCount: number;
  channelId: string | null;
  channelType: string | null | undefined;
  draftKey: string | null | undefined;
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
  // Subscription routes (Codex / Claude Code CLI) take ~5 s, so Enter waits
  // longer for them than for an API key.
  const sendWaitMs = routerModel?.sendWaitMs;
  const getRoster = useRouterRosterSource({
    enabled: routerActive,
    getIdentities: mentions.getMentionIdentities,
    memberPubkeys: mentions.memberPubkeys,
    selfPubkey,
  });
  const { getDraftMentionRefs } = mentions;
  const getExplicitMentionCount = React.useCallback(
    (text: string) => getDraftMentionRefs(text).length,
    [getDraftMentionRefs],
  );
  return useAutoAssign({
    addressedAgentCount,
    channelId,
    channelType,
    draftKey,
    getExplicitMentionCount,
    getRoster,
    isEditing,
    routerActive,
    routerReady,
    sendWaitMs,
    threadRoot,
  });
}
