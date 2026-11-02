import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// Schema check for the shared user-stream rules (test-fixtures/user-stream.json,
// docs/message-sync.md#user-stream). The rules themselves are tested by the
// clients' user-stream tests; this only keeps the fixture well formed.
const fixture = JSON.parse(
  readFileSync(
    new URL("../../../../test-fixtures/user-stream.json", import.meta.url),
    "utf8",
  ),
);

const NAMED_SECTIONS = [
  "part1",
  "storeSkip",
  "closed",
  "backoff",
  "busyRetry",
  "epoch",
  "snapshot",
  "skipCoverage",
  "batching",
  "ruleW",
  "newRow",
  "skipVerification",
  "auditSampling",
  "auditVerdicts",
  "liveMissed",
  "lastFrame",
  "comparator",
  "handoff",
  "midConnectionClosed",
  "membership",
  "routing",
];

test("every section has uniquely named cases", () => {
  for (const section of NAMED_SECTIONS) {
    const cases = fixture[section];
    assert.ok(Array.isArray(cases) && cases.length > 0, section);
    const names = cases.map((testCase) => testCase.name);
    assert.ok(
      names.every((name) => typeof name === "string" && name.length > 0),
      section,
    );
    assert.equal(new Set(names).size, names.length, `${section}: duplicate name`);
  }
});

test("the frozen kinds are sorted, unique and match the request", () => {
  const kinds = fixture.frozenKinds;
  assert.deepEqual([...new Set(kinds)].sort((a, b) => a - b), kinds);
  assert.deepEqual(fixture.request, { "#h": ["$joined"], kinds, limit: 0 });
});

test("every mode is described once", () => {
  assert.deepEqual(
    fixture.modes.map((mode) => mode.mode),
    ["shadow", "live", "gated"],
  );
});

test("snapshot parts are wire strings of kind 24220 or ignored kinds", () => {
  for (const testCase of fixture.part1) {
    assert.equal(typeof testCase.eose, "boolean", testCase.name);
    assert.equal(typeof testCase.expect.accept, "boolean", testCase.name);
    for (const event of testCase.events) {
      assert.equal(typeof event.content, "string", testCase.name);
      assert.equal(Number.isInteger(event.kind), true, testCase.name);
    }
  }
});

function closedKey(reason, mode) {
  const rule = fixture.closedReasons.find(({ match, value }) =>
    match === "prefix" ? reason.startsWith(value) : reason === value,
  );
  if (!rule) return "other";
  if (rule.key !== "overflow") return rule.key;
  return mode === "shadow" ? "shadow_full" : "full";
}

test("every CLOSED case's key follows the reason table", () => {
  const actions = new Set(["chunks", "fallback", "retryAfterAuth"]);
  for (const testCase of fixture.closed) {
    assert.equal(
      testCase.expect.key,
      closedKey(testCase.reason, testCase.mode),
      testCase.name,
    );
    assert.ok(actions.has(testCase.expect.action), testCase.name);
    if (testCase.expect.action === "fallback") {
      assert.ok(["live", "gated"].includes(testCase.mode), testCase.name);
      assert.equal(testCase.streamLive, true, testCase.name);
    }
  }
});

test("batches hold at most the batch size and are sorted by resume point", () => {
  const size = fixture.limits.catchUpBatchRooms;
  for (const testCase of fixture.batching) {
    const rooms = testCase.expect.flatMap((batch) => batch.rooms);
    const byResume = (a, b) =>
      testCase.resume[a] - testCase.resume[b] || (a < b ? -1 : a > b ? 1 : 0);
    assert.deepEqual([...rooms].sort(byResume), rooms, testCase.name);
    assert.equal(rooms.length, Object.keys(testCase.resume).length, testCase.name);
    for (const batch of testCase.expect) {
      assert.ok(batch.rooms.length <= size, testCase.name);
    }
  }
});
