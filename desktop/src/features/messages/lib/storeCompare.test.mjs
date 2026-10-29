import assert from "node:assert/strict";
import test from "node:test";

import { storeRelayDifference } from "./storeCompare.ts";

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
