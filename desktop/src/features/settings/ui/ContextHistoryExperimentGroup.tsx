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
import { SegmentedControl } from "@/shared/ui/segmented-control";
import { SettingsOptionGroup, SettingsOptionRow } from "./SettingsOptionGroup";

/**
 * Settings › Experiments › "New session history": how much earlier
 * conversation an agent reads when it starts a fresh session for a channel
 * main timeline or a thread. Resumed sessions and channel hosts keep the
 * recent messages. Running agents apply a change to their next new session.
 */
export function ContextHistoryExperimentGroup() {
  const query = useContextHistoryQuery();
  const setting = query.data ?? DEFAULT_CONTEXT_HISTORY_SETTING;
  const save = useSetContextHistoryMutation();
  // Never save edits made on top of the placeholder default.
  const locked = save.isPending || query.data === undefined;

  return (
    <SettingsOptionGroup
      data-testid="settings-context-history"
      description="What an agent reads when it starts a new session for a channel or thread, for example when its previous session could not be resumed. Resumed sessions and channel hosts keep the recent messages. Applies to the next new session, without a restart."
      title="New session history"
    >
      <SettingsOptionRow>
        <div className="min-w-0">
          <p className="font-medium text-foreground">Reads</p>
          <p
            className="mt-0.5 text-sm text-muted-foreground/70"
            data-settings-subcopy
          >
            Recent 12 reads only the latest messages
          </p>
        </div>
        <SegmentedControl
          disabled={locked}
          legend="New session history"
          onValueChange={(mode) =>
            save.mutate(selectContextHistoryMode(setting, mode))
          }
          optionTestIdPrefix="context-history-mode"
          options={CONTEXT_HISTORY_MODE_OPTIONS}
          testId="context-history-mode"
          value={setting.mode}
        />
      </SettingsOptionRow>
      <SettingsOptionRow>
        <div className="min-w-0">
          <p className="font-medium text-foreground">Size budget</p>
          <p
            className="mt-0.5 text-sm text-muted-foreground/70"
            data-settings-subcopy
          >
            {contextHistoryBudgetDescription(setting)}
          </p>
        </div>
        <SegmentedControl
          disabled={locked}
          legend="History size budget"
          onValueChange={(budget) =>
            save.mutate(selectContextHistoryBudget(budget))
          }
          optionTestIdPrefix="context-history-budget"
          options={CONTEXT_HISTORY_BUDGET_OPTIONS}
          testId="context-history-budget"
          value={setting.budget}
        />
      </SettingsOptionRow>
      {save.error ? (
        <p className="px-4 py-3 text-sm text-destructive" role="alert">
          {save.error instanceof Error
            ? save.error.message
            : "Couldn't save the new session history setting."}
        </p>
      ) : null}
    </SettingsOptionGroup>
  );
}
