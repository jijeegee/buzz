import assert from "node:assert/strict";
import test from "node:test";

import {
  goalForThread,
  goalLinkView,
  goalOutline,
  goalListTops,
  goalPath,
  goalSubtreeIds,
  goalTitleError,
  isPlanGoal,
  mergeGoalTrees,
  messageGoalLinkScope,
  threadGoalRootId,
} from "./goalTree.ts";

const node = (id, parent, extra = {}) => ({
  id,
  parent,
  title: id,
  note: "",
  status: "open",
  order: 0,
  assignees: [],
  links: [],
  matched: true,
  is_thread: false,
  ...extra,
});

const tree = {
  nodes: [
    node("b", "root", { order: 1 }),
    node("root", null),
    node("a", "root", { order: 0, links: ["ff".repeat(32)] }),
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
  assert.deepEqual([...goalSubtreeIds(tree, "a")].sort(), ["a", "a1", "a2"]);
  assert.equal(goalForThread(tree, "FF".repeat(32))?.id, "a");
  assert.equal(goalForThread(tree, "ee".repeat(32)), null);
});

test("plan goals sit above thread goals and unfold; thread goals do not", () => {
  const mixed = {
    nodes: [
      node("root", null),
      node("t", "root", { order: 0, is_thread: true }),
      node("plan", "root", { order: 1 }),
      node("task", "t"),
    ],
  };
  assert.deepEqual(
    goalOutline(mixed).map((row) => row.node.id),
    ["root", "plan", "t", "task"],
  );
  assert.equal(isPlanGoal(mixed, "plan"), true);
  assert.equal(isPlanGoal(mixed, "t"), false);
  assert.equal(isPlanGoal(mixed, "task"), false);
});

test("reads merge by goal, the last one winning", () => {
  const merged = mergeGoalTrees([
    { nodes: [node("a", null, { title: "old" }), node("b", "a")] },
    { nodes: [node("a", null, { title: "new" }), node("c", "a")] },
  ]);
  assert.deepEqual(
    merged.nodes.map((n) => [n.id, n.title]),
    [
      ["a", "new"],
      ["b", "b"],
      ["c", "c"],
    ],
  );
});

test("a list shows its topmost selected goals, not goals below them", () => {
  const list = {
    nodes: [
      node("root", null, { matched: false }),
      node("a", "root", { order: 1 }),
      node("mid", "a", { matched: false }),
      node("below", "mid"),
      node("b", "root", { order: 0 }),
    ],
  };
  assert.deepEqual(
    goalListTops(list).map((n) => n.id),
    ["b", "a"],
  );
});

test("empty tree has no outline", () => {
  assert.deepEqual(goalOutline({ nodes: [] }), []);
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

test("message link scope: thread branch when linked, else layer 2+", () => {
  const thread = "ff".repeat(32);
  const inLinked = messageGoalLinkScope(tree, thread);
  assert.deepEqual(
    goalOutline(tree)
      .filter(({ layer, node: n }) => inLinked.canLink(n.id, layer))
      .map(({ node: n }) => n.id),
    ["a", "a1", "a2"],
  );
  assert.equal(inLinked.canAddUnder("b"), false);
  const inUnlinked = messageGoalLinkScope(tree, "ee".repeat(32));
  assert.equal(inUnlinked.canLink("root", 1), false);
  assert.equal(inUnlinked.canLink("b", 2), true);
  const main = messageGoalLinkScope(tree, null);
  assert.equal(main.canLink("root", 1), false);
  assert.equal(main.canLink("b1", 3), true);
  assert.equal(main.canAddUnder("root"), true);
});

test("link view: threads open, messages jump, missing go nowhere", () => {
  const id = "aa".repeat(32);
  const root = "bb".repeat(32);
  assert.deepEqual(goalLinkView(id, undefined), {
    label: "Thread",
    target: { kind: "thread", threadRootId: id },
  });
  assert.deepEqual(goalLinkView(id, { kind: "thread" }).label, "Thread");
  assert.deepEqual(goalLinkView(id, { kind: "message" }), {
    label: "Message",
    target: { kind: "message", messageId: id, threadRootId: null },
  });
  assert.deepEqual(goalLinkView(id, { kind: "reply", threadRootId: root }), {
    label: "Message",
    target: { kind: "message", messageId: id, threadRootId: root },
  });
  assert.deepEqual(goalLinkView(id, { kind: "missing" }), {
    label: "Message",
    target: null,
  });
});
