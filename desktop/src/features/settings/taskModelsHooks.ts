import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  getTaskModels,
  MESSAGE_ROUTING_TASK_ID,
  setTaskModel,
  type TaskModelStatus,
} from "@/shared/api/tauriMessageRouting";

export const taskModelsQueryKey = ["task-models"] as const;

/**
 * Task model states. Readiness depends on API keys saved in the Providers
 * tab, so every mount refetches (staleTime 0) rather than trusting a cache
 * from before a key was added.
 */
export function useTaskModelsQuery() {
  return useQuery({
    queryKey: taskModelsQueryKey,
    queryFn: getTaskModels,
    staleTime: 0,
  });
}

/** The Smart routing task's state, or undefined while loading. */
export function useMessageRoutingModelQuery() {
  const query = useTaskModelsQuery();
  const status: TaskModelStatus | undefined = query.data?.find(
    (task) => task.taskId === MESSAGE_ROUTING_TASK_ID,
  );
  return { ...query, status };
}

/** One user action = one `set_task_model`. */
export function useSetTaskModelMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      taskId,
      provider,
      model,
    }: {
      taskId: string;
      provider: string | null;
      model: string | null;
    }) => setTaskModel(taskId, provider, model),
    onSuccess: (statuses) => {
      queryClient.setQueryData(taskModelsQueryKey, statuses);
    },
  });
}
