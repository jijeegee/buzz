import assert from "node:assert/strict";
import test from "node:test";

import {
  logStoreComparison,
  resetStoreComparisonTally,
  storeComparisonTally,
  storeRelayDifference,
  replyCountsOf,
  storeRelayOverlayDifference,
} from "./storeCompare.ts";

const row = (id, created_at) => ({ id, created_at });

test("rows inside the shared range are compared; live and paged-off rows are not", () => {
  const stored = [row("a", 10), row("b", 20), row("gone", 25), row("c", 30)];
  const relay = [row("b", 20), row("late", 22), row("c", 30), row("live", 40)];
  assert.deepEqual(storeRelayDifference(stored, relay), {
    compared: 3,
    missing: ["late"],
    extra: ["gone"],
  });
});

test("identical answers match", () => {
  const rows = [row("a", 1), row("b", 2)];
  assert.deepEqual(storeRelayDifference(rows, rows), {
    compared: 2,
    missing: [],
    extra: [],
  });
});

test("an empty answer on either side compares nothing", () => {
  assert.equal(storeRelayDifference([], [row("a", 1)]), null);
  assert.equal(storeRelayDifference([row("a", 1)], []), null);
});

const event = (id, created_at, kind, tags = [], content = "") => ({
  id,
  created_at,
  kind,
  tags,
  content,
});

test("overlays and summaries of shared rows are compared", () => {
  const stored = [
    event("e1", 50, 40003, [["e", "a"]]),
    event("s1", 60, 39005, [["e", "a"]], '{"reply_count":2}'),
    event("x", 70, 7, [["e", "other"]]),
  ];
  const relay = [
    event("r1", 55, 7, [["e", "a"]]),
    event("s2", 61, 39005, [["e", "a"]], '{"reply_count":3}'),
  ];
  const compare = (fence) =>
    storeRelayOverlayDifference(
      stored,
      relay,
      new Set(["a"]),
      replyCountsOf(stored),
      replyCountsOf(relay),
      fence,
    );
  assert.deepEqual(compare(100), {
    overlaysMissing: ["r1"],
    overlaysExtra: ["e1"],
    summaries: ["a"],
  });
  // Nothing newer than the store's newest event is compared.
  assert.deepEqual(compare(52), {
    overlaysMissing: [],
    overlaysExtra: ["e1"],
    summaries: [],
  });
});

test("an overlay its own answer removes is not a difference", () => {
  const stored = [
    event("re", 50, 7, [["e", "a"]]),
    event("del", 51, 5, [["e", "re"]]),
  ];
  const counts = new Map();
  assert.deepEqual(
    storeRelayOverlayDifference(stored, [], new Set(["a"]), counts, counts, 99),
    { overlaysMissing: [], overlaysExtra: [], summaries: [] },
  );
});

test("only reply counts of summaries are compared", () => {
  const summary = (id, content) =>
    event(id, 60, 39005, [["e", "a"]], JSON.stringify(content));
  const stored = [summary("s1", { reply_count: 2, participants: ["x"] })];
  const relay = [summary("s2", { reply_count: 2, participants: [] })];
  assert.deepEqual(
    storeRelayOverlayDifference(
      stored,
      relay,
      new Set(["a"]),
      replyCountsOf(stored),
      replyCountsOf(relay),
      99,
    ).summaries,
    [],
  );
});

test("comparisons are tallied by stream and type", (t) => {
  t.mock.method(console, "info", () => {});
  t.mock.method(console, "warn", () => {});
  resetStoreComparisonTally();
  const rows = [event("a", 1, 9), event("b", 2, 9)];
  logStoreComparison("room", "room", rows, rows, 0);
  logStoreComparison(
    "thread",
    "thread",
    rows,
    [...rows, event("r", 2, 7, [["e", "a"]])],
    1,
  );
  assert.deepEqual(storeComparisonTally(), {
    room: { matches: 1, differs: 0 },
    thread: { matches: 0, differs: 1 },
    byType: { missing: 0, extra: 0, overlays: 1, summaries: 0 },
  });
});
