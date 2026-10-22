import { useT } from "@/shared/i18n";

import { TASK_MODEL_TASKS } from "../../lib/taskModels";
import { useTaskModelsQuery } from "../../taskModelsHooks";
import {
  SettingsOptionGroup,
  SettingsOptionGroupList,
} from "../SettingsOptionGroup";
import { TaskModelRow } from "./TaskModelRow";

/**
 * Settings › Models › Task models.
 *
 * One row per entry in `TASK_MODEL_TASKS`, each showing the Rust-reported
 * state from `get_task_models` (saved choice, effective model, readiness).
 * A task Rust does not report renders nothing rather than an invented row.
 */
export function TaskModelsSettingsTab({
  onOpenProviders,
}: {
  onOpenProviders: () => void;
}) {
  const t = useT();
  const query = useTaskModelsQuery();
  const statuses = query.data ?? [];

  return (
    <div data-testid="settings-models-tasks">
      <SettingsOptionGroupList>
        <SettingsOptionGroup
          description={t("settings.taskModels.description")}
          title={t("settings.taskModels.title")}
        >
          {query.isError ? (
            <p className="px-4 py-3 text-sm text-destructive" role="alert">
              {t("settings.taskModels.loadFailed")}
            </p>
          ) : null}
          {TASK_MODEL_TASKS.map((task) => {
            const status = statuses.find((entry) => entry.taskId === task.id);
            return status ? (
              <TaskModelRow
                key={task.id}
                onOpenProviders={onOpenProviders}
                status={status}
                task={task}
              />
            ) : null;
          })}
        </SettingsOptionGroup>
      </SettingsOptionGroupList>
    </div>
  );
}
