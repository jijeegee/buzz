import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { buildObserverLiveFilter } from "./observerLiveFilter.ts";
import { meScopeOf } from "./relayMeBundle.ts";

const fixture = JSON.parse(
  readFileSync(
    new URL("../../../../test-fixtures/agent-observer.json", import.meta.url),
    "utf8",
  ),
);

test("shared fixture: the observer filter matches mobile", () => {
  assert.deepEqual(
    buildObserverLiveFilter(fixture.ownerPubkey),
    fixture.filter,
  );
});

test("the observer filter rides the addressed account bundle", () => {
  assert.deepEqual(meScopeOf(buildObserverLiveFilter(fixture.ownerPubkey)), {
    kind: "addressed",
    pubkey: fixture.ownerPubkey,
  });
});
