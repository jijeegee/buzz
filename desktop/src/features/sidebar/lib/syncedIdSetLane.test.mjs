import assert from "node:assert/strict";
import test from "node:test";

import { decodeDoc, encodeDoc } from "./sidebarLaneStore.ts";
import { mergeTrees } from "./sidebarLwwMap.ts";
import {
  idSetFromTree,
  PEOPLE_FAVORITES_LANE,
  setIdInTree,
  THREAD_PINS_LANE,
} from "./syncedIdSetLane.ts";

test("favorites add and remove one person at a time", () => {
  let tree = {};
  tree = setIdInTree(tree, "bob", true);
  tree = setIdInTree(tree, "alice", true);
  assert.deepEqual(idSetFromTree(tree), ["alice", "bob"]);
  tree = setIdInTree(tree, "bob", false);
  assert.deepEqual(idSetFromTree(tree), ["alice"]);
});

test("removing an unknown id or re-adding a member changes nothing", () => {
  const empty = {};
  assert.equal(setIdInTree(empty, "x", false), empty);
  const one = setIdInTree(empty, "x", true);
  assert.equal(setIdInTree(one, "x", true), one);
});

test("a later removal on another device wins the merge", () => {
  const added = setIdInTree({}, "carol", true);
  const removed = setIdInTree(added, "carol", false);
  assert.deepEqual(idSetFromTree(mergeTrees(added, removed)), []);
  assert.deepEqual(idSetFromTree(mergeTrees(removed, added)), []);
});

test("lanes round-trip through the synced document and stay separate", () => {
  const tree = setIdInTree({}, "root-1", true);
  const doc = encodeDoc(THREAD_PINS_LANE, tree);
  assert.deepEqual(doc.ids, ["root-1"]);
  const decoded = decodeDoc(THREAD_PINS_LANE, doc, 1);
  assert.deepEqual(idSetFromTree(decoded.tree), ["root-1"]);
  assert.notEqual(THREAD_PINS_LANE.dTag, PEOPLE_FAVORITES_LANE.dTag);
  assert.notEqual(
    THREAD_PINS_LANE.storageKey("pk", "wss://relay.example"),
    PEOPLE_FAVORITES_LANE.storageKey("pk", "wss://relay.example"),
  );
  assert.equal(decodeDoc(PEOPLE_FAVORITES_LANE, { version: 1 }, 1), null);
});
