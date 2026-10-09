import assert from "node:assert/strict";
import test from "node:test";

import { handleGoalsMockCommand, resetGoalsMock } from "./e2eBridgeGoals.ts";

const THREAD = "ab".repeat(32);
const run = (command, payload) =>
  handleGoalsMockCommand(command, payload)?.value;

test("goal commands used by the goals UI are mocked", () => {
  resetGoalsMock();
  const channelId = "c1";
  assert.deepEqual(run("get_goal_tree", { channelId }).tree.nodes, []);
  run("apply_goal_op", {
    channelId,
    op: { op: "set_root", id: "r", title: "Ship" },
  });
  const out = run("apply_goal_ops", {
    channelId,
    ops: [
      { op: "add", id: "g", parent: "r", title: "Goal" },
      { op: "link", id: "g", thread: THREAD },
    ],
  });
  const goal = out.tree.nodes.find((n) => n.id === "g");
  assert.deepEqual(goal.threads, [THREAD]);
  assert.equal(goal.status, "in_progress", "linking starts an open goal");
  assert.equal(run("get_goal_tree", { channelId }).tree.nodes.length, 2);
  assert.deepEqual(run("get_goal_tree_history", { channelId }), {
    revisions: [],
  });
  assert.equal(handleGoalsMockCommand("not_a_goal_command", {}), undefined);
});
