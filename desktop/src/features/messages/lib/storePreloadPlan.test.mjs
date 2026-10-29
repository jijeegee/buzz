import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { planStorePreload, storePreloadSummary } from "./storePreloadPlan.ts";

// Shared rules with mobile (test-fixtures/message-store-preload-plan.json).
const fixture = JSON.parse(
  readFileSync(
    new URL(
      "../../../../../test-fixtures/message-store-preload-plan.json",
      import.meta.url,
    ),
    "utf8",
  ),
);

for (const testCase of fixture.cases) {
  test(testCase.name, () => {
    const plan = planStorePreload(
      testCase.rooms,
      new Set(testCase.tokens),
      new Set(testCase.certain),
    );
    assert.deepEqual(
      {
        check: plan.check,
        certain: plan.certain,
        fetch: plan.fetch,
        no_token: plan.noToken,
      },
      testCase.expect,
    );
  });
}

test("one log line per pass", () => {
  const plan = planStorePreload(
    ["a", "b", "c"],
    new Set(["a", "b"]),
    new Set(["a"]),
  );
  assert.equal(
    storePreloadSummary(plan),
    "[preload] certain 1 / fetched 2 / skipped (no token) 1",
  );
});
