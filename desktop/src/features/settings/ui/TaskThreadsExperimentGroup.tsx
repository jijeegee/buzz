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
  const query = useTaskThreadsQuery();
  const setting = query.data ?? DEFAULT_TASK_THREADS_SETTING;
  const save = useSetTaskThreadsMutation();
  const enabled = new Set(effectiveTaskThreadTriggers(setting));
  // Never save edits made on top of the placeholder default.
  const locked = save.isPending || query.data === undefined;

  return (
    <SettingsOptionGroup
      data-testid="settings-task-threads"
      description="When agents move work into its own thread without being asked. Applies to agents whose conversation context is Each thread, from their next message."
      title="Agents open task threads"
    >
      <SettingsOptionRow>
        <div className="min-w-0">
          <p className="font-medium text-foreground">Level</p>
          <p
            className="mt-0.5 text-sm text-muted-foreground/70"
            data-settings-subcopy
          >
            Off keeps threads to when someone asks for one
          </p>
        </div>
        <SegmentedControl
          disabled={locked}
          legend="Task thread level"
          onValueChange={(level) =>
            save.mutate(selectTaskThreadLevel(setting, level))
          }
          optionTestIdPrefix="task-threads-level"
          options={TASK_THREAD_LEVEL_OPTIONS}
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
                {trigger.label}
              </label>
              <p
                className="mt-0.5 text-sm text-muted-foreground/70"
                data-settings-subcopy
              >
                {trigger.description}
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
            : "Couldn't save the task thread setting."}
        </p>
      ) : null}
    </SettingsOptionGroup>
  );
}
