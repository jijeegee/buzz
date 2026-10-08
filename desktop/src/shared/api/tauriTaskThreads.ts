import { invokeTauri } from "@/shared/api/tauri";

/** A situation in which an agent may open a task thread without being asked. */
export type TaskThreadTrigger =
  | "long_running"
  | "multi_step"
  | "parallel"
  | "side_discussion"
  | "delegation";

/** How readily agents open task threads; every level but `custom` is a preset. */
export type TaskThreadLevel =
  | "off"
  | "minimal"
  | "standard"
  | "proactive"
  | "custom";

export type TaskThreadsSetting = {
  level: TaskThreadLevel;
  /** Read for `custom` only; preset levels resolve to their preset. */
  triggers: TaskThreadTrigger[];
};

export async function getTaskThreads(): Promise<TaskThreadsSetting> {
  return invokeTauri<TaskThreadsSetting>("get_task_threads");
}

export async function setTaskThreads(
  setting: TaskThreadsSetting,
): Promise<TaskThreadsSetting> {
  return invokeTauri<TaskThreadsSetting>("set_task_threads", { setting });
}
