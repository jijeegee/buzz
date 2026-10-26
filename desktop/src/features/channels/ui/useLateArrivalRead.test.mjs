import assert from "node:assert/strict";
import { test } from "node:test";

import { lateArrivals } from "./useLateArrivalRead.ts";

const ME = "a".repeat(64);
const OTHER = "b".repeat(64);
const event = (id, pubkey, created_at) => ({ id, pubkey, created_at });

test("a message from someone else dated at or before the read is late", () => {
  const seen = new Set(["old"]);
  assert.deepEqual(
    lateArrivals(
      [
        event("old", OTHER, 90),
        event("late", OTHER, 95),
        event("same", OTHER, 100),
        event("newer", OTHER, 101),
        event("mine", ME.toUpperCase(), 99),
      ],
      seen,
      100,
      ME,
    ),
    ["late", "same"],
  );
  // Each event is judged once.
  assert.deepEqual(lateArrivals([event("late", OTHER, 95)], seen, 100, ME), []);
});
