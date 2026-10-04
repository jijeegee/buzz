import { TASK_MODEL_TASKS } from "../../lib/taskModels";
import {
  SettingsOptionGroup,
  SettingsOptionGroupList,
  SettingsOptionRow,
} from "../SettingsOptionGroup";

/**
 * Settings › Models › Task models.
 *
 * Renders one row per entry in `TASK_MODEL_TASKS`. The registry is empty
 * today, so the tab shows a deliberate empty state rather than inventing
 * tasks; the first task that lands brings its own `ModelEffortFields` row
 * and `task-models.json` persistence (see `lib/taskModels.ts`).
 */
export function TaskModelsSettingsTab() {
  return (
    <div data-testid="settings-models-tasks">
      <SettingsOptionGroupList>
        <SettingsOptionGroup
          description="Each task Buzz runs on its own behalf picks its provider, model, and effort here, independently of your agents."
          title="App tasks"
        >
          {TASK_MODEL_TASKS.length === 0 ? (
            <div
              className="px-4 py-6 text-sm"
              data-testid="settings-models-tasks-empty"
            >
              <p className="font-medium text-foreground">No app tasks yet</p>
              <p
                className="mt-1 text-sm text-muted-foreground/70"
                data-settings-subcopy
              >
                When Buzz starts using AI for its own work — summaries, titles,
                search — each task's provider, model, and effort will be set
                here.
              </p>
            </div>
          ) : (
            TASK_MODEL_TASKS.map((task) => (
              <SettingsOptionRow
                data-testid={`settings-models-task-${task.id}`}
                key={task.id}
              >
                <div className="min-w-0">
                  <p className="font-medium text-foreground">{task.label}</p>
                  <p
                    className="mt-0.5 text-sm text-muted-foreground/70"
                    data-settings-subcopy
                  >
                    {task.description}
                  </p>
                </div>
              </SettingsOptionRow>
            ))
          )}
        </SettingsOptionGroup>
      </SettingsOptionGroupList>
    </div>
  );
}
