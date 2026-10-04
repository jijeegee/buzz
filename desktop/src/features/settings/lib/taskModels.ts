/**
 * Registry of the AI tasks Buzz itself performs (summaries, titles, search…),
 * each of which gets its own provider / model / effort under
 * Settings › Models › Task models.
 *
 * Today the list is empty: Buzz does not yet run AI for its own work, and the
 * tab renders an explicit empty state instead of inventing rows. The first
 * task to land appends itself here; its persistence
 * (`<app-data>/agents/task-models.json`, one task = one save) and the
 * `ModelEffortFields`-based row ship in that same change. Task models are
 * deliberately not `GlobalAgentConfig` fields — saving that record restarts
 * running local agents, which an app-task change must never do.
 */
export type TaskModelTask = {
  /** Stable identifier; the future `task-models.json` key. */
  id: string;
  /** Row title, e.g. "Channel summaries". */
  label: string;
  /** One line on what the task does with the model. */
  description: string;
  /**
   * Effort the task starts on before the user picks one: `lowest` for cheap
   * background work, `harness-default` to leave the adapter's own default.
   */
  defaultEffortTier: "lowest" | "harness-default";
};

export const TASK_MODEL_TASKS: readonly TaskModelTask[] = [];

/** Ids must be unique: they key the future per-task persistence. */
export function duplicateTaskModelIds(
  tasks: readonly TaskModelTask[],
): string[] {
  const seen = new Set<string>();
  const duplicates = new Set<string>();
  for (const task of tasks) {
    if (seen.has(task.id)) duplicates.add(task.id);
    seen.add(task.id);
  }
  return [...duplicates];
}
