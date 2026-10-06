/**
 * Registry of the AI tasks Buzz itself performs, each of which gets its own
 * provider / model under Settings › Models › Task models.
 *
 * Persistence is Rust-owned: `<app-data>/agents/task-models.json`, one task
 * = one `set_task_model` save, read back through `get_task_models` (which
 * also reports readiness from the Providers tab's API keys). Task models are
 * deliberately not `GlobalAgentConfig` fields — saving that record restarts
 * running local agents, which an app-task change must never do. Rust keeps
 * the matching id list (`task_models::KNOWN_TASK_IDS`) and rejects others.
 */
export type TaskModelTask = {
  /** Stable identifier; the `task-models.json` key. */
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

export const TASK_MODEL_TASKS: readonly TaskModelTask[] = [
  {
    id: "message-routing",
    label: "Message routing",
    description:
      "Picks which agent handles a message you send without an @mention (Smart routing).",
    // The router never thinks: its row offers provider and model only.
    defaultEffortTier: "lowest",
  },
];

/** Ids must be unique: they key the per-task persistence. */
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
