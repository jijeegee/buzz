import { invokeTauri } from "@/shared/api/tauri";

/** What an agent reads when it starts a fresh main-timeline or thread session. */
export type ContextHistoryMode = "recent" | "budget";

/** Size budget for `budget` mode (about 8k / 20k / 50k tokens). */
export type ContextHistoryBudget = "small" | "medium" | "large";

export type ContextHistorySetting = {
  mode: ContextHistoryMode;
  /** Read for `budget` only; kept for `recent` so switching back restores it. */
  budget: ContextHistoryBudget;
};

export async function getContextHistory(): Promise<ContextHistorySetting> {
  return invokeTauri<ContextHistorySetting>("get_context_history");
}

export async function setContextHistory(
  setting: ContextHistorySetting,
): Promise<ContextHistorySetting> {
  return invokeTauri<ContextHistorySetting>("set_context_history", { setting });
}
