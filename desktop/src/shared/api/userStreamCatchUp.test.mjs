import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  auditEligible,
  auditSample,
  auditVerdicts,
  chunkCatchUpRooms,
  chunkRoomsAfterHandoff,
  classifySnapshot,
  coverageBefore,
  isNewRow,
  liveMissed,
  midConnectionPlan,
  planCatchUpBatches,
  routeStreamFrame,
  StreamMembership,
  StreamSpan,
  verifySkips,
} from "./userStreamCatchUp.ts";
import { closedDecision } from "./userStreamRules.ts";

// Catch-up cases of test-fixtures/user-stream.json
// (docs/message-sync.md#user-stream).
const fixture = JSON.parse(
  readFileSync(
    new URL("../../../../test-fixtures/user-stream.json", import.meta.url),
    "utf8",
  ),
);

const map = (record) => new Map(Object.entries(record));
const sorted = (list) => [...list].sort();

for (const testCase of fixture.snapshot) {
  test(`snapshot: ${testCase.name}`, () => {
    const plan = classifySnapshot(
      testCase.mode,
      map(testCase.snapshot),
      map(testCase.stored),
      map(testCase.resume),
    );
    const { expect } = testCase;
    assert.deepEqual(Object.fromEntries(plan.classes), expect.classes);
    assert.deepEqual(sorted(plan.skip), expect.skip);
    assert.deepEqual(sorted(plan.predictedSkips), expect.predictedSkips);
    assert.deepEqual(sorted(plan.walk), expect.walk);
    assert.deepEqual(sorted(plan.clearedRows), expect.clearedRows);
    assert.deepEqual(sorted(plan.notFetched), expect.notFetched);
    for (const room of expect.untouchedRows) {
      assert.equal(plan.classes.has(room), false, room);
    }
  });
}

for (const testCase of fixture.connectionSpans) {
  test(`connectionSpans: ${testCase.name}`, () => {
    const { connectedAt, L, segments, expect } = testCase;
    const { resume, head } = coverageBefore(segments, connectedAt);
    const seq = testCase.seq ?? 1;
    const stored =
      testCase.storedSeq === undefined ? {} : { "room-a": testCase.storedSeq };
    const plan = classifySnapshot(
      testCase.mode ?? "live",
      map({ "room-a": seq }),
      map(stored),
      resume === null ? new Map() : map({ "room-a": resume }),
    );
    const batch =
      resume === null
        ? undefined
        : planCatchUpBatches(map({ "room-a": resume }), L).batches[0];
    assert.equal(plan.classes.get("room-a"), expect.class);
    assert.equal(resume, expect.resume);
    assert.equal(batch?.since ?? null, expect.since);
    assert.deepEqual(head, expect.head);
    if (!testCase.walkRows) return;
    const newRows = testCase.walkRows.filter((row) =>
      isNewRow({
        liveAt: L,
        segments: head ? [head] : [],
        inserted: true,
        createdAt: row.created_at,
      }),
    ).length;
    const verdict = verifySkips(
      plan.predictedSkips.map(() => ({ walked: true, newRows })),
    );
    assert.equal(plan.predictedSkips.length, expect.predictedSkips);
    assert.equal(verdict.falseSkip, expect.falseSkip);
  });
}

for (const testCase of fixture.batching) {
  test(`batching: ${testCase.name}`, () => {
    assert.deepEqual(
      planCatchUpBatches(map(testCase.resume), testCase.L),
      testCase.expect,
    );
  });
}

for (const testCase of fixture.newRow) {
  test(`newRow: ${testCase.name}`, () => {
    assert.equal(
      isNewRow({
        liveAt: testCase.L,
        segments: testCase.segments,
        inserted: testCase.row.inserted,
        createdAt: testCase.row.created_at,
        receivedLive: testCase.row.receivedLive,
      }),
      testCase.counts,
    );
  });
}

for (const testCase of fixture.skipVerification) {
  test(`skipVerification: ${testCase.name}`, () => {
    assert.deepEqual(verifySkips(testCase.rooms), testCase.expect);
  });
}

for (const testCase of fixture.auditSampling) {
  test(`auditSampling: ${testCase.name}`, () => {
    const eligible = auditEligible(map(testCase.skipped), testCase.L);
    assert.deepEqual(eligible, testCase.expect.eligible);
    const sample = auditSample(eligible);
    assert.equal(sample.length, testCase.expect.sampleSize);
    assert.equal(new Set(sample).size, sample.length);
    assert.ok(sample.every((room) => eligible.includes(room)));
  });
}

for (const testCase of fixture.auditVerdicts) {
  test(`auditVerdicts: ${testCase.name}`, () => {
    assert.deepEqual(auditVerdicts(testCase.batches), testCase.expect);
  });
}

for (const testCase of fixture.liveMissed) {
  test(`liveMissed: ${testCase.name}`, () => {
    assert.deepEqual(
      liveMissed({
        prev: testCase.prev,
        walked: map(testCase.walked),
        relayPubkey: testCase.relayPubkey,
        rows: testCase.rows.map((row) => ({
          room: row.room,
          kind: row.kind,
          pubkey: row.pubkey,
          inserted: row.inserted,
          createdAt: row.created_at,
          liveAt: testCase.L,
          segments: testCase.segments[row.room] ?? [],
        })),
      }),
      testCase.expect,
    );
  });
}

for (const testCase of fixture.handoff) {
  test(`handoff: ${testCase.name}`, () => {
    const rooms = [...testCase.memberRooms, ...testCase.quietRooms];
    assert.deepEqual(rooms, testCase.expect.beforeEose);
    assert.deepEqual(
      chunkRoomsAfterHandoff({
        mode: testCase.mode,
        streamLive: testCase.mode !== null && !testCase.closedBeforeEose,
        rooms,
        snapshotRooms: new Set(testCase.snapshotRooms),
      }),
      testCase.expect.afterEose,
    );
  });
}

for (const testCase of fixture.chunkCatchUp) {
  test(`chunkCatchUp: ${testCase.name}`, () => {
    assert.deepEqual(
      chunkCatchUpRooms(
        testCase.stream,
        testCase.chunkRooms,
        new Set(testCase.snapshotRooms),
      ),
      testCase.expect.chunkCatchUpRooms,
    );
  });
}

for (const testCase of fixture.midConnectionClosed) {
  test(`midConnectionClosed: ${testCase.name}`, () => {
    const decision = closedDecision({
      reason: testCase.reason ?? "error: user-stream closed",
      mode: testCase.mode,
      streamLive: testCase.streamLive,
    });
    const plan = midConnectionPlan({
      mode: testCase.mode,
      streamLive: testCase.streamLive,
      awaitSocketClose: decision.action === "awaitSocketClose",
      closedAt: testCase.closedAt,
      carriedRooms: testCase.carriedRooms,
      chunkExtras: testCase.chunkExtras,
      resume: map(testCase.resume),
      lastSeenCreatedAt: testCase.stream.lastSeenCreatedAt,
      liveSince: map(testCase.stream.liveSince),
    });
    const { streamAgainThisConnection, ...expect } = testCase.expect;
    assert.equal(streamAgainThisConnection, false);
    assert.deepEqual(plan, expect);
  });
}

for (const testCase of fixture.spanStop) {
  test(`spanStop: ${testCase.name}`, () => {
    const span = new StreamSpan();
    span.eose(testCase.streamEoseAt);
    for (const event of testCase.events) {
      if (event.closed) span.closed();
      else if (event.on === "stream") span.frame(event.at);
    }
    assert.deepEqual(span.span, testCase.expect.streamSpan);
  });
}

for (const testCase of fixture.membership) {
  test(`membership: ${testCase.name}`, () => {
    const membership = new StreamMembership(fixture.membershipCarriedBefore);
    const stored = [];
    const rendered = [];
    const coverage = {};
    for (const step of testCase.steps) {
      if (step.op === "44101") membership.removed(step.room, step.created_at);
      else if (step.op === "44100") {
        if (testCase.beforeEose) continue;
        const from = membership.joined(
          step.room,
          step.created_at,
          step.receivedAt,
        );
        // As mobile's runner: the EOSE catch-up is already done here, so a
        // rejoin writes its span at once (a pending room waits for it).
        if (from !== null) coverage[step.room] = [[from, from]];
      } else {
        stored.push(step.id);
        if (membership.renders(step.room, step.created_at)) {
          rendered.push(step.id);
        }
      }
    }
    assert.deepEqual(stored, testCase.expect.stored);
    assert.deepEqual(rendered, testCase.expect.rendered);
    assert.deepEqual(sorted(membership.carried), testCase.expect.carried);
    if (testCase.expect.coverage) {
      assert.deepEqual(coverage, testCase.expect.coverage);
    }
  });
}

for (const testCase of fixture.routing) {
  test(`routing: ${testCase.name}`, () => {
    const lists = fixture.routingLists;
    assert.deepEqual(
      routeStreamFrame(testCase.frame.room, {
        memberRooms: new Set(lists.memberRooms),
        archivedRooms: new Set(lists.archivedRooms),
        openRooms: new Set(lists.openRooms),
      }),
      testCase.expect,
    );
  });
}
