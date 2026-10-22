import {
  DEFAULT_TASK_THREADS_SETTING,
  effectiveTaskThreadTriggers,
  selectTaskThreadLevel,
  TASK_THREAD_LEVEL_OPTIONS,
  TASK_THREAD_TRIGGERS,
  toggleTaskThreadTrigger,
} from "@/features/settings/lib/taskThreadsSetting";
import {
  useSetTaskThreadsMutation,
  useTaskThreadsQuery,
} from "@/features/settings/taskThreadsHooks";
import { useT } from "@/shared/i18n";
import { Checkbox } from "@/shared/ui/checkbox";
import { SegmentedControl } from "@/shared/ui/segmented-control";
import { SettingsOptionGroup, SettingsOptionRow } from "./SettingsOptionGroup";

/**
 * Settings › Experiments › "Agents open task threads": how readily agents
 * with the "Each thread" conversation context move work into a task thread
 * without being asked. A level presets the situations; ticking one directly
 * makes the level Custom. Running agents apply a change on their next message.
 */
export function TaskThreadsExperimentGroup() {
  const t = useT();
  const query = useTaskThreadsQuery();
  const setting = query.data ?? DEFAULT_TASK_THREADS_SETTING;
  const save = useSetTaskThreadsMutation();
  const enabled = new Set(effectiveTaskThreadTriggers(setting));
  // Never save edits made on top of the placeholder default.
  const locked = save.isPending || query.data === undefined;

  return (
    <SettingsOptionGroup
      data-testid="settings-task-threads"
      description={t("settings.taskThreads.description")}
      title={t("settings.taskThreads.title")}
    >
      <SettingsOptionRow>
        <div className="min-w-0">
          <p className="font-medium text-foreground">
            {t("settings.taskThreads.levelLabel")}
          </p>
          <p
            className="mt-0.5 text-sm text-muted-foreground/70"
            data-settings-subcopy
          >
            {t("settings.taskThreads.levelHint")}
          </p>
        </div>
        <SegmentedControl
          disabled={locked}
          legend={t("settings.taskThreads.levelLegend")}
          onValueChange={(level) =>
            save.mutate(selectTaskThreadLevel(setting, level))
          }
          optionTestIdPrefix="task-threads-level"
          options={TASK_THREAD_LEVEL_OPTIONS.map((option) => ({
            value: option.value,
            label: t(option.label),
          }))}
          size="wide"
          testId="task-threads-level"
          value={setting.level}
        />
      </SettingsOptionRow>
      {TASK_THREAD_TRIGGERS.map((trigger) => {
        const id = `task-threads-trigger-${trigger.value}`;
        return (
          <SettingsOptionRow data-testid={id} key={trigger.value}>
            <div className="min-w-0">
              <label className="font-medium text-foreground" htmlFor={id}>
                {t(trigger.label)}
              </label>
              <p
                className="mt-0.5 text-sm text-muted-foreground/70"
                data-settings-subcopy
              >
                {t(trigger.description)}
              </p>
            </div>
            <Checkbox
              checked={enabled.has(trigger.value)}
              disabled={locked}
              id={id}
              onCheckedChange={(checked) =>
                save.mutate(
                  toggleTaskThreadTrigger(
                    setting,
                    trigger.value,
                    checked === true,
                  ),
                )
              }
            />
          </SettingsOptionRow>
        );
      })}
      {save.error ? (
        <p className="px-4 py-3 text-sm text-destructive" role="alert">
          {save.error instanceof Error
            ? save.error.message
            : t("settings.taskThreads.saveFailed")}
        </p>
      ) : null}
    </SettingsOptionGroup>
  );
}
