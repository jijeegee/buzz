import {
  useContextHistoryQuery,
  useSetContextHistoryMutation,
} from "@/features/settings/contextHistoryHooks";
import {
  CONTEXT_HISTORY_BUDGET_OPTIONS,
  CONTEXT_HISTORY_MODE_OPTIONS,
  contextHistoryBudgetDescription,
  DEFAULT_CONTEXT_HISTORY_SETTING,
  selectContextHistoryBudget,
  selectContextHistoryMode,
} from "@/features/settings/lib/contextHistorySetting";
import { useT } from "@/shared/i18n";
import { SegmentedControl } from "@/shared/ui/segmented-control";
import { SettingsOptionGroup, SettingsOptionRow } from "./SettingsOptionGroup";

/**
 * Settings › Experiments › "New session history": how much earlier
 * conversation an agent reads when it starts a fresh session for a channel
 * main timeline or a thread. Resumed sessions and channel hosts keep the
 * recent messages. Running agents apply a change to their next new session.
 */
export function ContextHistoryExperimentGroup() {
  const t = useT();
  const query = useContextHistoryQuery();
  const setting = query.data ?? DEFAULT_CONTEXT_HISTORY_SETTING;
  const save = useSetContextHistoryMutation();
  // Never save edits made on top of the placeholder default.
  const locked = save.isPending || query.data === undefined;

  return (
    <SettingsOptionGroup
      data-testid="settings-context-history"
      description={t("settings.contextHistory.description")}
      title={t("settings.contextHistory.title")}
    >
      <SettingsOptionRow>
        <div className="min-w-0">
          <p className="font-medium text-foreground">
            {t("settings.contextHistory.reads")}
          </p>
          <p
            className="mt-0.5 text-sm text-muted-foreground/70"
            data-settings-subcopy
          >
            {t("settings.contextHistory.readsHint")}
          </p>
        </div>
        <SegmentedControl
          disabled={locked}
          legend={t("settings.contextHistory.title")}
          onValueChange={(mode) =>
            save.mutate(selectContextHistoryMode(setting, mode))
          }
          optionTestIdPrefix="context-history-mode"
          options={CONTEXT_HISTORY_MODE_OPTIONS.map((option) => ({
            value: option.value,
            label: t(option.label),
          }))}
          testId="context-history-mode"
          value={setting.mode}
        />
      </SettingsOptionRow>
      <SettingsOptionRow>
        <div className="min-w-0">
          <p className="font-medium text-foreground">
            {t("settings.contextHistory.sizeBudget")}
          </p>
          <p
            className="mt-0.5 text-sm text-muted-foreground/70"
            data-settings-subcopy
          >
            {contextHistoryBudgetDescription(setting)}
          </p>
        </div>
        <SegmentedControl
          disabled={locked}
          legend={t("settings.contextHistory.sizeBudgetLegend")}
          onValueChange={(budget) =>
            save.mutate(selectContextHistoryBudget(budget))
          }
          optionTestIdPrefix="context-history-budget"
          options={CONTEXT_HISTORY_BUDGET_OPTIONS.map((option) => ({
            value: option.value,
            label: t(option.label),
          }))}
          testId="context-history-budget"
          value={setting.budget}
        />
      </SettingsOptionRow>
      {save.error ? (
        <p className="px-4 py-3 text-sm text-destructive" role="alert">
          {save.error instanceof Error
            ? save.error.message
            : t("settings.contextHistory.saveFailed")}
        </p>
      ) : null}
    </SettingsOptionGroup>
  );
}
