import assert from "node:assert/strict";
import test from "node:test";

import { handleGoalsMockCommand, resetGoalsMock } from "./e2eBridgeGoals.ts";

const THREAD = "ab".repeat(32);
const run = (command, payload) =>
  handleGoalsMockCommand(command, payload)?.value;

test("goal commands used by the goals UI are mocked", () => {
  resetGoalsMock();
  const channelId = "c1";
  const active = () => run("get_goals", { channelId, view: "active" });
  assert.deepEqual(active().rows, []);
  run("apply_goal_ops", {
    channelId,
    ops: [{ op: "set_root", id: "r", title: "Ship" }],
  });
  run("apply_goal_ops", {
    channelId,
    ops: [
      { op: "add", id: "g", parent: "r", title: "Goal" },
      { op: "link", id: "g", thread: THREAD },
      { op: "add", id: "t", parent: "g", title: "Task" },
    ],
  });
  const goal = active().rows.find((n) => n.id === "g");
  assert.deepEqual(goal.links, [THREAD]);
  assert.equal(goal.status, "in_progress", "linking starts an open goal");
  assert.equal(goal.is_thread, true);
  assert.deepEqual(goal.progress, { done: 0, total: 1 });
  assert.equal(active().rows.length, 3);

  run("apply_goal_ops", {
    channelId,
    ops: [{ op: "update", id: "g", status: "done" }],
  });
  const done = run("get_goals", { channelId, view: "done" });
  assert.equal(done.counts.done, 1);
  assert.deepEqual(
    done.rows.map((n) => [n.id, n.matched]),
    [
      ["r", false],
      ["g", true],
    ],
  );
  const thread = run("get_thread_goals", { channelId, threads: [THREAD] });
  assert.deepEqual(thread.threads, [{ thread: THREAD, goal: "g" }]);
  assert.deepEqual(run("get_goal_history", { channelId }), { edits: [] });
  assert.equal(handleGoalsMockCommand("not_a_goal_command", {}), undefined);
});
