import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { ROOM_LIVE_KINDS } from "./relayRoomBundle.ts";
import {
  busyRetryDelaySecs,
  closedDecision,
  lastFrameAt,
  NetworkChangeFilter,
  namesUserStream,
  parsePlainIntegers,
  roomIdsOf,
  ShadowComparator,
  SnapshotParser,
  USER_STREAM_KINDS,
  USER_STREAM_LIMITS,
  userStreamFilter,
  UserStreamBackoff,
} from "./userStreamRules.ts";

// Runs the shared cases of test-fixtures/user-stream.json against the
// desktop rules (docs/message-sync.md#user-stream).
const fixture = JSON.parse(
  readFileSync(
    new URL("../../../../test-fixtures/user-stream.json", import.meta.url),
    "utf8",
  ),
);

test("the frozen kinds and the REQ match the fixture and the room chunks", () => {
  assert.deepEqual([...USER_STREAM_KINDS], fixture.frozenKinds);
  assert.deepEqual(userStreamFilter(), fixture.request);
  assert.deepEqual(
    [...new Set(ROOM_LIVE_KINDS)].sort((a, b) => a - b),
    fixture.frozenKinds,
  );
});

test("the limits match the fixture", () => {
  for (const [key, value] of Object.entries(USER_STREAM_LIMITS)) {
    if (key in fixture.limits) {
      assert.deepEqual(value, fixture.limits[key], key);
    }
  }
  assert.deepEqual(
    fixture.limits.busyRetryDelaysSecs.map((_, attempt) =>
      busyRetryDelaySecs(attempt),
    ),
    fixture.limits.busyRetryDelaysSecs,
  );
});

test("$joined is never read as a room id", () => {
  assert.equal(namesUserStream(userStreamFilter()), true);
  assert.equal(namesUserStream({ "#h": ["room-a"] }), false);
  assert.deepEqual(roomIdsOf({ "#h": ["$joined", "room-a"] }), ["room-a"]);
  assert.equal(roomIdsOf({}), undefined);
});

for (const testCase of fixture.part1) {
  test(`part1: ${testCase.name}`, () => {
    const parser = new SnapshotParser();
    for (const event of testCase.events) parser.feed(event);
    const snapshot = testCase.eose ? parser.eose() : null;
    assert.equal(snapshot !== null, testCase.expect.accept);
    if (!testCase.expect.accept) return;
    assert.equal(snapshot.mode, testCase.expect.mode);
    assert.equal(snapshot.epoch, testCase.expect.epoch);
    assert.deepEqual(Object.fromEntries(snapshot.rooms), testCase.expect.rooms);
    assert.equal(parser.ignoredBeforePart1, testCase.expect.ignoredBeforePart1);
  });
}

test("part1: a seq above 2^53 - 1 is refused", () => {
  const parser = new SnapshotParser();
  parser.feed({
    kind: 24220,
    content: `{"v":1,"op":"snapshot","part":1,"parts":1,"mode":"live","epoch":"e1","kinds":${JSON.stringify(fixture.frozenKinds)},"rooms":{"a":9007199254740993}}`,
  });
  assert.equal(parser.eose(), null);
});

for (const testCase of fixture.closed) {
  test(`closed: ${testCase.name}`, () => {
    assert.deepEqual(
      closedDecision({
        reason: testCase.reason,
        mode: testCase.mode,
        streamLive: testCase.streamLive,
        afterRetry: testCase.afterRetry ?? false,
      }),
      testCase.expect,
    );
  });
}

function runBackoff(events, checks) {
  const backoff = new UserStreamBackoff();
  const timeline = [
    ...events.map((event) => ({ ...event, kind: "event" })),
    ...checks.map((check) => ({ ...check, kind: "check" })),
  ].sort((a, b) => a.at - b.at || (a.kind === "event" ? -1 : 1));
  for (const step of timeline) {
    if (step.kind === "check") {
      assert.equal(
        backoff.allowed(step.at),
        step.streamAllowed,
        `at ${step.at}`,
      );
    } else if (step.op === "closed") {
      if (["full", "closed", "other"].includes(step.key)) {
        backoff.noteClose(step.at);
      }
    } else if (step.op === "networkChange") {
      backoff.networkChange();
    } else if (step.op === "resume") {
      backoff.resume(step.backgroundSecs);
    }
  }
}

for (const testCase of fixture.backoff) {
  test(`backoff: ${testCase.name}`, () => {
    runBackoff(testCase.events, testCase.checks);
  });
}

test("backoff: only a real change of network types is a network change", () => {
  const filter = new NetworkChangeFilter();
  assert.equal(filter.changed(["wifi"]), false, "the first report after start");
  assert.equal(filter.changed(["wifi"]), false, "a repeat");
  assert.equal(filter.changed(["cellular", "wifi"]), true);
  assert.equal(filter.changed(["wifi", "cellular"]), false, "same set");
  assert.equal(filter.changed([]), false, "no network");
  assert.equal(filter.changed(["wifi"]), true);
});

for (const testCase of fixture.busyRetry.filter((c) => c.busyCloses)) {
  test(`busyRetry: ${testCase.name}`, () => {
    const delays = Array.from({ length: testCase.busyCloses }, (_, attempt) =>
      busyRetryDelaySecs(attempt),
    );
    assert.deepEqual(delays, testCase.expect.delaysSecs);
  });
}

for (const testCase of fixture.lastFrame) {
  test(`lastFrame: ${testCase.name}`, () => {
    assert.equal(lastFrameAt(testCase.frames), testCase.lastFrameAt);
  });
}

for (const testCase of fixture.comparator) {
  test(`comparator: ${testCase.name}`, () => {
    const chunkRooms = new Set(testCase.chunkRooms);
    const comparator = new ShadowComparator(
      testCase.snapshotRooms,
      () => chunkRooms,
    );
    comparator.streamEose(testCase.streamEoseAtMs);
    const timeline = [
      ...testCase.joined.map((joined) => ({ ...joined, joined: true })),
      ...testCase.observations,
    ].sort((a, b) => a.atMs - b.atMs);
    for (const step of timeline) {
      if (step.joined) comparator.noteJoined(step.room);
      else if (step.closed) comparator.closed();
      else comparator.observe(step.on, step.id, step.room, step.atMs);
    }
    comparator.end(testCase.endAtMs);
    assert.deepEqual(comparator.counters, testCase.expect);
  });
}

test("comparator: a chunk-only event is told apart by age and kind", () => {
  const comparator = new ShadowComparator(["a"], () => new Set(["a"]));
  comparator.streamEose(100_000);
  comparator.observe("chunk", "old", "a", 101_000, {
    kind: 9,
    created_at: 50,
  });
  comparator.observe("chunk", "new", "a", 101_000, {
    kind: 7,
    created_at: 101,
  });
  comparator.end(200_000);
  assert.equal(comparator.counters.chunkOnly, 2);
  assert.deepEqual(comparator.chunkOnlyDetail, {
    chunkOnlyOld: 1,
    chunkOnlyKind9: 1,
    chunkOnlyNew: 1,
    chunkOnlyKind7: 1,
  });
});

// ── The session runtime (relayUserStream.ts) ────────────────────────────────

const { UserStreamSession } = await import("./relayUserStream.ts");

function part1(mode, rooms = { "room-a": 3 }, extra = {}) {
  return {
    id: `p1-${mode}`,
    kind: 24220,
    pubkey: "relay",
    created_at: 100,
    tags: [],
    content: JSON.stringify({
      v: 1,
      op: "snapshot",
      part: 1,
      parts: 1,
      mode,
      epoch: "e1",
      kinds: fixture.frozenKinds,
      rooms,
      ...extra,
    }),
  };
}

function fakeHost({ chunkRooms = ["room-a"], enabled = true } = {}) {
  const host = {
    now: 1_000_000,
    gen: 1,
    sent: [],
    closedSockets: [],
    timers: new Map(),
    nextTimer: 1,
    logs: [],
    generation: () => host.gen,
    send: async (payload, generation) => {
      host.sent.push({ payload, generation });
    },
    closeSocket: (generation) => host.closedSockets.push(generation),
    chunkRooms: () => new Set(chunkRooms),
    roomOf: (event) => event.tags.find((tag) => tag[0] === "h")?.[1] ?? null,
    nowMs: () => host.now,
    setTimeout: (fn, ms) => {
      const id = host.nextTimer++;
      host.timers.set(id, { fn, at: host.now + ms });
      return id;
    },
    clearTimeout: (id) => host.timers.delete(id),
    log: (line) => host.logs.push(line),
    enabled: () => enabled,
    random: () => 0.5,
    advance(ms) {
      host.now += ms;
      for (const [id, timer] of [...host.timers]) {
        if (timer.at <= host.now) {
          host.timers.delete(id);
          timer.fn();
        }
      }
    },
  };
  return host;
}

const roomEvent = (id, room) => ({
  id,
  kind: 9,
  pubkey: "a",
  created_at: 1000,
  tags: [["h", room]],
  content: "",
});

test("session: the stream REQ goes out once per connection with the frozen filter", async () => {
  const host = fakeHost();
  const session = new UserStreamSession(host);
  await session.connected(1);
  assert.equal(host.sent.length, 1);
  const [label, subId, filter] = host.sent[0].payload;
  assert.equal(label, "REQ");
  assert.ok(subId.startsWith("stream-"));
  assert.deepEqual(filter, fixture.request);
  assert.equal(session.handles(subId), true);
});

test("session: the build flag off sends nothing", async () => {
  const host = fakeHost({ enabled: false });
  const session = new UserStreamSession(host);
  await session.connected(1);
  assert.deepEqual(host.sent, []);
});

test("session: an old relay's CLOSED leaves chunks alone and never retries", async () => {
  const host = fakeHost();
  const session = new UserStreamSession(host);
  await session.connected(1);
  const subId = host.sent[0].payload[1];
  session.closed(subId, "restricted: not a channel member");
  assert.equal(host.sent.length, 1);
  assert.deepEqual(host.closedSockets, []);
  assert.equal(session.report.counters.closed_restricted, 1);
});

test("session: shadow compares the stream with the chunks from its EOSE", async () => {
  const host = fakeHost();
  const session = new UserStreamSession(host);
  await session.connected(1);
  const subId = host.sent[0].payload[1];
  session.event(subId, part1("shadow"));
  session.eose(subId);
  session.chunkEvent(roomEvent("e1", "room-a"), "room-a");
  session.event(subId, roomEvent("e1", "room-a"));
  session.event(subId, roomEvent("e2", "room-x"));
  host.now += 6_000;
  session.socketEnded();
  assert.equal(session.report.counters.eventsCompared, 1);
  assert.equal(session.report.counters.streamOnlyUnexpected, 1);
  assert.ok(
    host.logs.some((line) => line.startsWith("[user-stream] report #1 ")),
  );
});

test("session: live part 1 without the live handoff closes the stream (chunks)", async () => {
  const host = fakeHost();
  const session = new UserStreamSession(host);
  await session.connected(1);
  const subId = host.sent[0].payload[1];
  session.event(subId, part1("live"));
  assert.deepEqual(host.sent[1].payload, ["CLOSE", subId]);
  session.event(subId, roomEvent("late", "room-a"));
  session.eose(subId);
  assert.equal(session.connection.streamLive, false);
});

test("session: a refused snapshot closes the stream", async () => {
  const host = fakeHost();
  const session = new UserStreamSession(host);
  await session.connected(1);
  const subId = host.sent[0].payload[1];
  session.event(subId, part1("shadow", {}, { parts: 2 }));
  session.eose(subId);
  assert.deepEqual(host.sent[1].payload, ["CLOSE", subId]);
  assert.equal(session.report.counters.snapshotRefused, 1);
});

test("session: auth-required is retried once on the same id after AUTH OK", async () => {
  const host = fakeHost();
  const session = new UserStreamSession(host);
  await session.connected(1);
  const subId = host.sent[0].payload[1];
  session.closed(subId, "auth-required: authenticate first");
  session.authOk(1);
  assert.deepEqual(host.sent[1].payload[1], subId);
  session.closed(subId, "auth-required: authenticate first");
  session.authOk(1);
  assert.equal(host.sent.length, 2);
});

test("session: lagged waits for the socket, ends it after 2 s, then jitters", async () => {
  const host = fakeHost();
  const session = new UserStreamSession(host);
  await session.connected(1);
  const subId = host.sent[0].payload[1];
  session.event(subId, part1("shadow"));
  session.eose(subId);
  session.closed(subId, "error: user-stream lagged");
  host.advance(1_999);
  assert.deepEqual(host.closedSockets, []);
  host.advance(1);
  assert.deepEqual(host.closedSockets, [1]);
  assert.equal(session.takeReconnectJitterMs(), 5_000);
  assert.equal(session.takeReconnectJitterMs(), 0);
});

test("session: a socket that closes within the grace is not closed again", async () => {
  const host = fakeHost();
  const session = new UserStreamSession(host);
  await session.connected(1);
  const subId = host.sent[0].payload[1];
  session.closed(subId, "error: user-stream lagged");
  session.socketEnded();
  host.gen = 2;
  host.advance(5_000);
  assert.deepEqual(host.closedSockets, []);
});

test("session: two counting closes back the stream off; a network change ends it", async () => {
  const host = fakeHost();
  const session = new UserStreamSession(host);
  for (let close = 0; close < 2; close += 1) {
    await session.connected(close + 1);
    const subId = host.sent.at(-1).payload[1];
    session.closed(subId, "error: user-stream overflow");
    session.socketEnded();
  }
  const sent = host.sent.length;
  await session.connected(3);
  assert.equal(host.sent.length, sent, "backed off");
  const saved = session.backoff.save();
  const restarted = new UserStreamSession(host);
  restarted.backoff.restore(saved);
  await restarted.connected(4);
  assert.equal(host.sent.length, sent, "survives a restart");
  session.networkChange();
  await session.connected(5);
  assert.equal(host.sent.length, sent + 1);
});

test("session: two accounts keep their own modes and verdicts", async () => {
  const hostA = fakeHost();
  const hostB = fakeHost();
  const a = new UserStreamSession(hostA);
  const b = new UserStreamSession(hostB);
  await a.connected(1);
  await b.connected(1);
  a.event(hostA.sent[0].payload[1], part1("shadow"));
  b.event(hostB.sent[0].payload[1], part1("live"));
  a.eose(hostA.sent[0].payload[1]);
  assert.equal(a.connection.mode, "shadow");
  assert.equal(a.connection.streamLive, true);
  assert.equal(b.connection.mode, "live");
  assert.equal(b.connection.closed, true);
});

test("only plain digit numbers survive the integer read; strings are untouched", () => {
  assert.deepEqual(
    parsePlainIntegers(
      '{"a":0,"b":12,"c":1.0,"d":1e0,"e":-0,"f":-3,"g":"1.0 \\" 2e5","h":[7,7.5]}',
    ),
    {
      a: 0,
      b: 12,
      c: null,
      d: null,
      e: null,
      f: null,
      g: '1.0 " 2e5',
      h: [7, null],
    },
  );
  assert.equal(parsePlainIntegers("{bad"), undefined);
  assert.equal(parsePlainIntegers('{"a":01}'), undefined);
});
