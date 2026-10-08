import type {
  ContextHistoryBudget,
  ContextHistoryMode,
  ContextHistorySetting,
} from "@/shared/api/tauriContextHistory";

export const CONTEXT_HISTORY_MODE_OPTIONS: readonly {
  value: ContextHistoryMode;
  label: string;
}[] = [
  { value: "recent", label: "Recent 12" },
  { value: "budget", label: "Full history" },
];

/** Display order and copy for each budget, matching the harness sizes. */
export const CONTEXT_HISTORY_BUDGET_OPTIONS: readonly {
  value: ContextHistoryBudget;
  label: string;
  description: string;
}[] = [
  { value: "small", label: "Small", description: "About 8k tokens" },
  { value: "medium", label: "Medium", description: "About 20k tokens" },
  { value: "large", label: "Large", description: "About 50k tokens" },
];

/** Mirrors Rust's default when no setting has been saved. */
export const DEFAULT_CONTEXT_HISTORY_SETTING: ContextHistorySetting = {
  mode: "recent",
  budget: "medium",
};

/** The harness value a setting sends; empty keeps the recent messages. */
export function contextHistoryEnvValue(setting: ContextHistorySetting): string {
  return setting.mode === "budget" ? setting.budget : "";
}

/** Picking a mode keeps the chosen budget so switching back restores it. */
export function selectContextHistoryMode(
  current: ContextHistorySetting,
  mode: ContextHistoryMode,
): ContextHistorySetting {
  return { ...current, mode };
}

/** Picking a budget always switches to full history within that budget. */
export function selectContextHistoryBudget(
  budget: ContextHistoryBudget,
): ContextHistorySetting {
  return { mode: "budget", budget };
}

/** The copy under the budget row for the current setting. */
export function contextHistoryBudgetDescription(
  setting: ContextHistorySetting,
): string {
  if (setting.mode === "recent") {
    return "Pick a size to read as much history as fits";
  }
  const option = CONTEXT_HISTORY_BUDGET_OPTIONS.find(
    (item) => item.value === setting.budget,
  );
  return `${option?.description ?? "Within the budget"}, oldest messages dropped first`;
}
