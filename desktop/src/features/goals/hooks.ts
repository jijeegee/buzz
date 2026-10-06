import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { relayClient } from "@/shared/api/relayClient";
import {
  applyGoalOp,
  type GoalOp,
  getGoalTree,
  getGoalTreeHistory,
  restoreGoalTree,
} from "@/shared/api/tauriGoals";

/** Kind 40110 — see `buzz_core::kind::KIND_GOAL_TREE`. */
export const KIND_GOAL_TREE = 40110;

const goalTreeKey = (channelId: string | null) => ["goal-tree", channelId];
const goalHistoryKey = (channelId: string | null) => [
  "goal-tree-history",
  channelId,
];

/**
 * The live goal tree of a channel or DM. A relay subscription refetches it
 * whenever anyone (human or agent) publishes a new revision.
 */
export function useGoalTreeQuery(channelId: string | null, enabled = true) {
  const queryClient = useQueryClient();
  const active = enabled && channelId !== null;

  React.useEffect(() => {
    if (!active || !channelId) return;
    let disposed = false;
    let cleanup: (() => void) | undefined;
    relayClient
      .subscribeLive(
        {
          kinds: [KIND_GOAL_TREE],
          "#h": [channelId],
          limit: 1,
          since: Math.floor(Date.now() / 1000),
        },
        () => {
          void queryClient.invalidateQueries({
            queryKey: goalTreeKey(channelId),
          });
          void queryClient.invalidateQueries({
            queryKey: goalHistoryKey(channelId),
          });
        },
      )
      .then((dispose) => {
        if (disposed) {
          void dispose();
          return;
        }
        cleanup = () => void dispose();
      })
      .catch((error) => {
        console.error("[goals] subscription failed:", error);
      });
    return () => {
      disposed = true;
      cleanup?.();
    };
  }, [active, channelId, queryClient]);

  return useQuery({
    queryKey: goalTreeKey(channelId),
    queryFn: () => {
      if (!channelId) return Promise.reject(new Error("No channel selected"));
      return getGoalTree(channelId);
    },
    enabled: active,
    staleTime: 30_000,
  });
}

export function useApplyGoalOpMutation(channelId: string | null) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (op: GoalOp) => {
      if (!channelId) return Promise.reject(new Error("No channel selected"));
      return applyGoalOp(channelId, op);
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: goalTreeKey(channelId) });
      void queryClient.invalidateQueries({
        queryKey: goalHistoryKey(channelId),
      });
    },
  });
}

export function useGoalHistoryQuery(
  channelId: string | null,
  enabled: boolean,
) {
  return useQuery({
    queryKey: goalHistoryKey(channelId),
    queryFn: () => {
      if (!channelId) return Promise.reject(new Error("No channel selected"));
      return getGoalTreeHistory(channelId);
    },
    enabled: enabled && channelId !== null,
  });
}

export function useRestoreGoalTreeMutation(channelId: string | null) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (revision: string) => {
      if (!channelId) return Promise.reject(new Error("No channel selected"));
      return restoreGoalTree(channelId, revision);
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: goalTreeKey(channelId) });
      void queryClient.invalidateQueries({
        queryKey: goalHistoryKey(channelId),
      });
    },
  });
}
