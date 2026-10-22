import type {
  ContextHistoryBudget,
  ContextHistoryMode,
  ContextHistorySetting,
} from "@/shared/api/tauriContextHistory";
import { type MessageKey, t } from "@/shared/i18n";

/** Labels are message keys; translate where shown. */
export const CONTEXT_HISTORY_MODE_OPTIONS: readonly {
  value: ContextHistoryMode;
  label: MessageKey;
}[] = [
  { value: "recent", label: "settings.contextHistory.modeRecent" },
  { value: "budget", label: "settings.contextHistory.modeFull" },
];

/**
 * Display order and copy for each budget, matching the harness sizes.
 * Labels and descriptions are message keys; translate where shown.
 */
export const CONTEXT_HISTORY_BUDGET_OPTIONS: readonly {
  value: ContextHistoryBudget;
  label: MessageKey;
  description: MessageKey;
}[] = [
  {
    value: "small",
    label: "settings.contextHistory.budgetSmall",
    description: "settings.contextHistory.budgetSmallDescription",
  },
  {
    value: "medium",
    label: "settings.contextHistory.budgetMedium",
    description: "settings.contextHistory.budgetMediumDescription",
  },
  {
    value: "large",
    label: "settings.contextHistory.budgetLarge",
    description: "settings.contextHistory.budgetLargeDescription",
  },
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
    return t("settings.contextHistory.budgetHintRecent");
  }
  const option = CONTEXT_HISTORY_BUDGET_OPTIONS.find(
    (item) => item.value === setting.budget,
  );
  return t(
    option?.description ?? "settings.contextHistory.budgetFallbackDescription",
  );
}
