import type {
  TaskThreadLevel,
  TaskThreadsSetting,
  TaskThreadTrigger,
} from "@/shared/api/tauriTaskThreads";
import type { MessageKey } from "@/shared/i18n";

/**
 * Display order and copy for each situation, matching the harness guidance.
 * Labels and descriptions are message keys; translate where shown.
 */
export const TASK_THREAD_TRIGGERS: readonly {
  value: TaskThreadTrigger;
  label: MessageKey;
  description: MessageKey;
}[] = [
  {
    value: "long_running",
    label: "settings.taskThreads.trigger.longRunning",
    description: "settings.taskThreads.trigger.longRunningDescription",
  },
  {
    value: "multi_step",
    label: "settings.taskThreads.trigger.multiStep",
    description: "settings.taskThreads.trigger.multiStepDescription",
  },
  {
    value: "parallel",
    label: "settings.taskThreads.trigger.parallel",
    description: "settings.taskThreads.trigger.parallelDescription",
  },
  {
    value: "side_discussion",
    label: "settings.taskThreads.trigger.sideDiscussion",
    description: "settings.taskThreads.trigger.sideDiscussionDescription",
  },
  {
    value: "delegation",
    label: "settings.taskThreads.trigger.delegation",
    description: "settings.taskThreads.trigger.delegationDescription",
  },
];

/** Labels are message keys; translate where shown. */
export const TASK_THREAD_LEVEL_OPTIONS: readonly {
  value: TaskThreadLevel;
  label: MessageKey;
}[] = [
  { value: "off", label: "settings.taskThreads.level.off" },
  { value: "minimal", label: "settings.taskThreads.level.minimal" },
  { value: "standard", label: "settings.taskThreads.level.standard" },
  { value: "proactive", label: "settings.taskThreads.level.proactive" },
  { value: "custom", label: "settings.taskThreads.level.custom" },
];

const PRESETS: Record<
  Exclude<TaskThreadLevel, "custom">,
  readonly TaskThreadTrigger[]
> = {
  off: [],
  minimal: ["long_running"],
  standard: ["long_running", "multi_step", "parallel"],
  proactive: [
    "long_running",
    "multi_step",
    "parallel",
    "side_discussion",
    "delegation",
  ],
};

/** Mirrors Rust's default when no setting has been saved. */
export const DEFAULT_TASK_THREADS_SETTING: TaskThreadsSetting = {
  level: "minimal",
  triggers: [...PRESETS.minimal],
};

/** The triggers a setting enables, in display order. */
export function effectiveTaskThreadTriggers(
  setting: TaskThreadsSetting,
): TaskThreadTrigger[] {
  const enabled = new Set(
    setting.level === "custom" ? setting.triggers : PRESETS[setting.level],
  );
  return TASK_THREAD_TRIGGERS.map((trigger) => trigger.value).filter((value) =>
    enabled.has(value),
  );
}

/**
 * Picking a level: a preset fills in its triggers; Custom keeps whatever is
 * currently enabled so the checkboxes do not jump.
 */
export function selectTaskThreadLevel(
  current: TaskThreadsSetting,
  level: TaskThreadLevel,
): TaskThreadsSetting {
  if (level === "custom") {
    return { level, triggers: effectiveTaskThreadTriggers(current) };
  }
  return { level, triggers: [...PRESETS[level]] };
}

/** Ticking or unticking a situation always makes the setting Custom. */
export function toggleTaskThreadTrigger(
  current: TaskThreadsSetting,
  trigger: TaskThreadTrigger,
  enabled: boolean,
): TaskThreadsSetting {
  const triggers = new Set(effectiveTaskThreadTriggers(current));
  if (enabled) {
    triggers.add(trigger);
  } else {
    triggers.delete(trigger);
  }
  return {
    level: "custom",
    triggers: TASK_THREAD_TRIGGERS.map((item) => item.value).filter((value) =>
      triggers.has(value),
    ),
  };
}
