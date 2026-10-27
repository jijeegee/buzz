import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  buildObserverLiveFilter,
  OBSERVER_LIVE_LIMIT,
  OBSERVER_LIVE_LOOKBACK_SECS,
} from "./observerLiveFilter.ts";

const fixture = JSON.parse(
  readFileSync(
    new URL("../../../../test-fixtures/agent-observer.json", import.meta.url),
    "utf8",
  ),
);

test("shared fixture: observer lookback and limit match mobile", () => {
  assert.equal(OBSERVER_LIVE_LOOKBACK_SECS, fixture.lookbackSecs);
  assert.equal(OBSERVER_LIVE_LIMIT, fixture.limit);
  assert.deepEqual(buildObserverLiveFilter("ab", 10_000), {
    kinds: [24200],
    "#p": ["ab"],
    limit: fixture.limit,
    since: 10_000 - fixture.lookbackSecs,
  });
});
