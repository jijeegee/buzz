import assert from "node:assert/strict";
import test from "node:test";

import {
  goalForThread,
  goalOutline,
  goalPath,
  goalProgress,
  goalSubtreeIds,
  goalTitleError,
  threadGoalRootId,
} from "./goalTree.ts";

const node = (id, parent, extra = {}) => ({
  id,
  parent,
  title: id,
  status: "open",
  order: 0,
  ...extra,
});

const tree = {
  v: 1,
  nodes: [
    node("b", "root", { order: 1 }),
    node("root", null),
    node("a", "root", { order: 0, threads: ["ff".repeat(32)] }),
    node("a1", "a", { status: "done" }),
    node("a2", "a", { order: 1 }),
    node("b1", "b"),
  ],
};

test("outline is depth-first with layers and sibling order", () => {
  assert.deepEqual(
    goalOutline(tree).map((row) => [row.layer, row.node.id]),
    [
      [1, "root"],
      [2, "a"],
      [3, "a1"],
      [3, "a2"],
      [2, "b"],
      [3, "b1"],
    ],
  );
});

test("path, progress, subtree, and thread lookup", () => {
  assert.deepEqual(
    goalPath(tree, "a2").map((n) => n.id),
    ["root", "a", "a2"],
  );
  assert.deepEqual(goalProgress(tree, "a"), { done: 1, total: 2 });
  assert.deepEqual(goalProgress(tree, "root"), { done: 1, total: 5 });
  assert.deepEqual([...goalSubtreeIds(tree, "a")].sort(), ["a", "a1", "a2"]);
  assert.equal(goalForThread(tree, "FF".repeat(32))?.id, "a");
  assert.equal(goalForThread(tree, "ee".repeat(32)), null);
});

test("empty tree has no outline", () => {
  assert.deepEqual(goalOutline({ v: 1, nodes: [] }), []);
});

test("title rules mirror the relay", () => {
  assert.equal(goalTitleError("Ship it"), null);
  assert.ok(goalTitleError("  "));
  assert.ok(goalTitleError("one\ntwo"));
  assert.ok(goalTitleError("x".repeat(201)));
});

test("threads link to goals by their chain root, like mobile and agents", () => {
  assert.equal(threadGoalRootId({ id: "head", rootId: "root" }), "root");
  assert.equal(threadGoalRootId({ id: "root", rootId: null }), "root");
  assert.equal(threadGoalRootId({ id: "root" }), "root");
});
