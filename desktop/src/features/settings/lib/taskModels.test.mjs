import assert from "node:assert/strict";
import test from "node:test";

import { duplicateTaskModelIds, TASK_MODEL_TASKS } from "./taskModels.ts";

test("task model ids are unique (they key task-models.json)", () => {
  assert.deepEqual(duplicateTaskModelIds(TASK_MODEL_TASKS), []);
  for (const task of TASK_MODEL_TASKS) {
    assert.ok(task.id.trim().length > 0, "task id must not be blank");
    assert.ok(task.label.trim().length > 0, `${task.id}: label required`);
    assert.ok(
      task.defaultEffortTier === "lowest" ||
        task.defaultEffortTier === "harness-default",
      `${task.id}: unknown effort tier ${task.defaultEffortTier}`,
    );
  }
});

test("message routing is a registered task (Smart routing's model row)", () => {
  const task = TASK_MODEL_TASKS.find((entry) => entry.id === "message-routing");
  assert.ok(task, "message-routing must be registered");
  assert.equal(task.label, "Message routing");
  assert.match(task.description, /without an @mention/);
});

test("duplicateTaskModelIds reports each repeated id once", () => {
  const task = (id) => ({
    id,
    label: id,
    description: "",
    defaultEffortTier: "lowest",
  });
  assert.deepEqual(
    duplicateTaskModelIds([
      task("summaries"),
      task("titles"),
      task("summaries"),
      task("summaries"),
    ]),
    ["summaries"],
  );
  assert.deepEqual(duplicateTaskModelIds([task("a"), task("b")]), []);
});
