import * as React from "react";
import {
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";

import { useUsersBatchQuery } from "@/features/profile/hooks";
import { roomLive } from "@/shared/api/relayRoomLive";
import { KIND_GOAL_OPS } from "@/shared/api/relayRoomLiveRegistry";
import {
  applyGoalOps,
  getActiveGoals,
  getGoalHistory,
  getGoalPage,
  getThreadGoals,
  type GoalCursor,
  type GoalOp,
  type GoalTree,
  type GoalView,
} from "@/shared/api/tauriGoals";
import { t as translate, useT } from "@/shared/i18n";
import { classifyGoalLinks } from "./goalLinks";
import { mergeGoalTrees } from "./goalTree";

/** Everything read about a room's goals, refreshed together. */
const goalsKey = (channelId: string | null) => ["goals", channelId];

const noRoom = () =>
  Promise.reject(new Error(translate("goals.noGroupSelected")));

/**
 * Reads again whenever anyone (human or agent) edits the room's goals, and
 * after a reconnect, which may have missed edits. The open room's own REQ
 * (shared with typing) carries the edits.
 */
function useGoalsLive(channelId: string | null, active: boolean) {
  const queryClient = useQueryClient();
  React.useEffect(() => {
    if (!active || !channelId) return;
    const refetch = () => {
      void queryClient.invalidateQueries({ queryKey: goalsKey(channelId) });
    };
    return roomLive.listen(channelId, KIND_GOAL_OPS, refetch, {
      onResync: refetch,
    });
  }, [active, channelId, queryClient]);
}

/** The active goals of a channel or DM, with the size of each list. */
export function useGoalTreeQuery(channelId: string | null, enabled = true) {
  const active = enabled && channelId !== null;
  useGoalsLive(channelId, active);
  return useQuery({
    queryKey: [...goalsKey(channelId), "active"],
    queryFn: async () => {
      if (!channelId) return noRoom();
      const read = await getActiveGoals(channelId);
      const tree: GoalTree = { nodes: read.rows };
      return { tree, counts: read.counts };
    },
    enabled: active,
    staleTime: 30_000,
  });
}

/**
 * The active goals plus what `ids` (threads or messages) work on, which may
 * be quiet or finished. Undefined until both are loaded.
 */
export function useThreadGoalTree(channelId: string | null, ids: string[]) {
  const activeQuery = useGoalTreeQuery(channelId);
  const focusQuery = useQuery({
    queryKey: [...goalsKey(channelId), "threads", ids.join(",")],
    queryFn: () => (channelId ? getThreadGoals(channelId, ids) : noRoom()),
    enabled: channelId !== null,
    staleTime: 30_000,
  });
  const active = activeQuery.data?.tree;
  const focus = focusQuery.data?.rows;
  return React.useMemo(
    () =>
      active && focus ? mergeGoalTrees([active, { nodes: focus }]) : undefined,
    [active, focus],
  );
}

/** A quiet or done list, read a page at a time. */
export function useGoalListQuery(channelId: string | null, view: GoalView) {
  useGoalsLive(channelId, channelId !== null);
  return useInfiniteQuery({
    queryKey: [...goalsKey(channelId), "list", view],
    queryFn: ({ pageParam }) =>
      channelId ? getGoalPage(channelId, view, pageParam) : noRoom(),
    initialPageParam: null as GoalCursor | null,
    getNextPageParam: (page) => page.next,
    enabled: channelId !== null,
    staleTime: 30_000,
  });
}

/** Apply one edit, or several as a single event (all or none). */
export function useApplyGoalOpMutation(channelId: string | null) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (op: GoalOp | GoalOp[]) =>
      channelId
        ? applyGoalOps(channelId, Array.isArray(op) ? op : [op])
        : noRoom(),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: goalsKey(channelId) });
    },
  });
}

/** Display names for everyone assigned to a goal in `tree`. */
export function useGoalAssigneeNames(tree: GoalTree | undefined) {
  const assigneePubkeys = React.useMemo(
    () => [...new Set((tree?.nodes ?? []).flatMap((n) => n.assignees))],
    [tree],
  );
  const usersQuery = useUsersBatchQuery(assigneePubkeys);
  const t = useT();
  return React.useCallback(
    (pubkey: string) =>
      usersQuery.data?.profiles[pubkey]?.displayName ??
      t("goals.unknownMember"),
    [usersQuery.data, t],
  );
}

/**
 * Whether each id linked in `tree` is a thread, a message, a reply, or
 * missing (see `classifyGoalLinks`). Undefined until loaded.
 */
export function useGoalLinkKinds(
  channelId: string | null,
  tree: GoalTree | undefined,
) {
  const ids = React.useMemo(
    () => [...new Set((tree?.nodes ?? []).flatMap((n) => n.links))].sort(),
    [tree],
  );
  return useQuery({
    queryKey: ["goal-link-kinds", channelId, ids.join(",")],
    queryFn: () => (channelId ? classifyGoalLinks(channelId, ids) : noRoom()),
    enabled: channelId !== null && ids.length > 0,
    staleTime: 30_000,
  }).data;
}

export function useGoalHistoryQuery(
  channelId: string | null,
  enabled: boolean,
) {
  return useQuery({
    queryKey: [...goalsKey(channelId), "history"],
    queryFn: () => (channelId ? getGoalHistory(channelId) : noRoom()),
    enabled: enabled && channelId !== null,
  });
}
