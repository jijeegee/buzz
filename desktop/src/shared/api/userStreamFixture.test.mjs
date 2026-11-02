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
  "spanStop",
  "chunkCatchUp",
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
  const actions = new Set([
    "chunks",
    "fallback",
    "awaitSocketClose",
    "retryAfterAuth",
  ]);
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
    const counts =
      ["full", "closed", "other"].includes(testCase.expect.key) &&
      ["live", "gated"].includes(testCase.mode) &&
      (testCase.streamLive || testCase.expect.key === "full");
    assert.equal(testCase.expect.countsTowardBackoff, counts, testCase.name);
  }
});

// A room whose walk would start more than 7 days before L is stale.
function isStale(resume, liveAt) {
  const { lookbackSecs, catchUpMaxAgeSecs } = fixture.limits;
  return resume - lookbackSecs < liveAt - catchUpMaxAgeSecs;
}

test("batches leave out stale rooms, then sort and split the rest", () => {
  const { catchUpBatchRooms: size, lookbackSecs } = fixture.limits;
  for (const testCase of fixture.batching) {
    const { resume, L } = testCase;
    const byResume = (a, b) =>
      resume[a] - resume[b] || (a < b ? -1 : a > b ? 1 : 0);
    const rooms = Object.keys(resume).sort(byResume);
    const stale = rooms.filter((room) => isStale(resume[room], L));
    const fresh = rooms.filter((room) => !isStale(resume[room], L));
    assert.deepEqual(testCase.expect.stale, [...stale].sort(), testCase.name);
    const batches = [];
    for (let i = 0; i < fresh.length; i += size) {
      const batch = fresh.slice(i, i + size);
      batches.push({ rooms: batch, since: resume[batch[0]] - lookbackSecs });
    }
    assert.deepEqual(testCase.expect.batches, batches, testCase.name);
  }
});

test("audit eligibility follows the age limit", () => {
  for (const testCase of fixture.auditSampling) {
    const eligible = Object.keys(testCase.skipped)
      .filter((room) => !isStale(testCase.skipped[room], testCase.L))
      .sort();
    assert.deepEqual(testCase.expect.eligible, eligible, testCase.name);
    assert.equal(
      testCase.expect.sampleSize,
      Math.min(eligible.length, fixture.limits.auditSampleMax),
      testCase.name,
    );
  }
});
