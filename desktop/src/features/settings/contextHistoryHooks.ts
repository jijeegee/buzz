import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { managedAgentsQueryKey } from "@/features/agents/hooks";
import {
  type ContextHistorySetting,
  getContextHistory,
  setContextHistory,
} from "@/shared/api/tauriContextHistory";

export const contextHistoryQueryKey = ["context-history"] as const;

export function useContextHistoryQuery() {
  return useQuery({
    queryKey: contextHistoryQueryKey,
    queryFn: getContextHistory,
    refetchOnWindowFocus: false,
  });
}

export function useSetContextHistoryMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (setting: ContextHistorySetting) => setContextHistory(setting),
    onMutate: async (setting) => {
      await queryClient.cancelQueries({ queryKey: contextHistoryQueryKey });
      const previous = queryClient.getQueryData<ContextHistorySetting>(
        contextHistoryQueryKey,
      );
      queryClient.setQueryData(contextHistoryQueryKey, setting);
      return { previous };
    },
    onError: (_error, _setting, context) => {
      if (context?.previous) {
        queryClient.setQueryData(contextHistoryQueryKey, context.previous);
      }
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: contextHistoryQueryKey });
      // Running agents read the setting at launch; refreshing the list
      // surfaces their restart badge right away.
      void queryClient.invalidateQueries({ queryKey: managedAgentsQueryKey });
    },
  });
}
