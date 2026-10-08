import assert from "node:assert/strict";
import test from "node:test";

import {
  DEFAULT_TASK_THREADS_SETTING,
  effectiveTaskThreadTriggers,
  selectTaskThreadLevel,
  toggleTaskThreadTrigger,
} from "./taskThreadsSetting.ts";

test("default is long-running work only, like the Rust default", () => {
  assert.deepEqual(effectiveTaskThreadTriggers(DEFAULT_TASK_THREADS_SETTING), [
    "long_running",
  ]);
});

test("presets ignore stored triggers; custom uses them in display order", () => {
  assert.deepEqual(
    effectiveTaskThreadTriggers({
      level: "standard",
      triggers: ["delegation"],
    }),
    ["long_running", "multi_step", "parallel"],
  );
  assert.deepEqual(
    effectiveTaskThreadTriggers({
      level: "custom",
      triggers: ["delegation", "long_running"],
    }),
    ["long_running", "delegation"],
  );
  assert.deepEqual(
    effectiveTaskThreadTriggers({ level: "off", triggers: ["parallel"] }),
    [],
  );
});

test("choosing Custom keeps the current selection", () => {
  assert.deepEqual(
    selectTaskThreadLevel({ level: "standard", triggers: [] }, "custom"),
    {
      level: "custom",
      triggers: ["long_running", "multi_step", "parallel"],
    },
  );
  assert.deepEqual(
    selectTaskThreadLevel({ level: "custom", triggers: ["delegation"] }, "off"),
    { level: "off", triggers: [] },
  );
});

test("ticking a situation switches to Custom", () => {
  assert.deepEqual(
    toggleTaskThreadTrigger(
      DEFAULT_TASK_THREADS_SETTING,
      "side_discussion",
      true,
    ),
    { level: "custom", triggers: ["long_running", "side_discussion"] },
  );
  assert.deepEqual(
    toggleTaskThreadTrigger(
      { level: "proactive", triggers: [] },
      "long_running",
      false,
    ),
    {
      level: "custom",
      triggers: ["multi_step", "parallel", "side_discussion", "delegation"],
    },
  );
});
