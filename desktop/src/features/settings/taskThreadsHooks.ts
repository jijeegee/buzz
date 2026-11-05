import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { managedAgentsQueryKey } from "@/features/agents/hooks";
import { useIdentityQuery } from "@/shared/api/hooks";
import {
  getTaskThreads,
  setTaskThreads,
  type TaskThreadsSetting,
} from "@/shared/api/tauriTaskThreads";

export const taskThreadsQueryKey = ["task-threads"] as const;

export function useTaskThreadsQuery() {
  return useQuery({
    queryKey: taskThreadsQueryKey,
    queryFn: getTaskThreads,
    refetchOnWindowFocus: false,
  });
}

export function useSetTaskThreadsMutation() {
  const queryClient = useQueryClient();
  const owner = useIdentityQuery().data?.pubkey;
  return useMutation({
    mutationFn: (setting: TaskThreadsSetting) => setTaskThreads(setting, owner),
    onMutate: async (setting) => {
      await queryClient.cancelQueries({ queryKey: taskThreadsQueryKey });
      const previous =
        queryClient.getQueryData<TaskThreadsSetting>(taskThreadsQueryKey);
      queryClient.setQueryData(taskThreadsQueryKey, setting);
      return { previous };
    },
    onError: (_error, _setting, context) => {
      if (context?.previous) {
        queryClient.setQueryData(taskThreadsQueryKey, context.previous);
      }
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: taskThreadsQueryKey });
      // Running agents read the setting at launch; refreshing the list
      // surfaces their restart badge right away.
      void queryClient.invalidateQueries({ queryKey: managedAgentsQueryKey });
    },
  });
}
