import type {
  TaskThreadLevel,
  TaskThreadsSetting,
  TaskThreadTrigger,
} from "@/shared/api/tauriTaskThreads";

/** Display order and copy for each situation, matching the harness guidance. */
export const TASK_THREAD_TRIGGERS: readonly {
  value: TaskThreadTrigger;
  label: string;
  description: string;
}[] = [
  {
    value: "long_running",
    label: "Long-running work",
    description:
      "Builds, deploys, test suites, or broad research that takes more than a few minutes",
  },
  {
    value: "multi_step",
    label: "Multi-step work",
    description:
      "Work that goes through several stages, like investigate, change, and verify",
  },
  {
    value: "parallel",
    label: "Several tasks in one request",
    description: "One thread per independent task",
  },
  {
    value: "side_discussion",
    label: "Side discussions",
    description: "Long tangents that would crowd out the main timeline",
  },
  {
    value: "delegation",
    label: "Delegating to another agent",
    description: "Keep the assignment, progress, and result in one thread",
  },
];

export const TASK_THREAD_LEVEL_OPTIONS: readonly {
  value: TaskThreadLevel;
  label: string;
}[] = [
  { value: "off", label: "Off" },
  { value: "minimal", label: "Light" },
  { value: "standard", label: "Standard" },
  { value: "proactive", label: "Eager" },
  { value: "custom", label: "Custom" },
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
