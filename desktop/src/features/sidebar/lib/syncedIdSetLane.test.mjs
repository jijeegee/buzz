import assert from "node:assert/strict";
import test from "node:test";

import { decodeDoc, encodeDoc } from "./sidebarLaneStore.ts";
import { mergeTrees } from "./sidebarLwwMap.ts";
import {
  idSetFromTree,
  MAX_ID_SET_TOMBSTONES,
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

test("only the newest removals are kept, so the record stays bounded", async () => {
  let tree = {};
  for (let i = 0; i < 60; i++) tree = setIdInTree(tree, `id${i}`, true);
  for (let i = 0; i < 60; i++) {
    tree = setIdInTree(tree, `id${i}`, false);
    // Distinct register versions per removal.
    await new Promise((resolve) => setTimeout(resolve, 2));
  }
  const kept = Object.keys(tree.i);
  assert.equal(kept.length, MAX_ID_SET_TOMBSTONES);
  assert.ok(kept.includes("id59"), "the newest removal is kept");
  assert.ok(!kept.includes("id0"), "the oldest removal is dropped");
  // A recent removal still wins over another device's older add.
  const otherDevice = { i: { id59: [1, "f".repeat(16), true] } };
  assert.deepEqual(idSetFromTree(mergeTrees(tree, otherDevice)), []);
});

test("pins match the relay's 200-thread cap; favorites use the same bound", async () => {
  assert.equal(THREAD_PINS_LANE.maxEntries, 200);
  assert.equal(PEOPLE_FAVORITES_LANE.maxEntries, 200);
  assert.equal(THREAD_PINS_LANE.withinLimits({ ids: new Array(201) }), false);
  // 200 ids plus the kept removals fit in the relay's event size limit.
  let tree = {};
  for (let i = 0; i < 200; i++) {
    tree = setIdInTree(tree, i.toString(16).padStart(64, "0"), true);
  }
  for (let i = 0; i < MAX_ID_SET_TOMBSTONES; i++) {
    const id = (1000 + i).toString(16).padStart(64, "0");
    tree = setIdInTree(setIdInTree(tree, id, true), id, false);
  }
  const plaintext = JSON.stringify(encodeDoc(THREAD_PINS_LANE, tree));
  // NIP-44 base64 adds about a third.
  assert.ok(plaintext.length * 1.4 < 65_535, `${plaintext.length} bytes`);
});
