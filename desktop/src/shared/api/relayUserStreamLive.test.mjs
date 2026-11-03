import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, test } from "node:test";

import { RoomLiveBundle } from "./relayRoomBundle.ts";
import {
  catchUpFromStore,
  setStoreCoverageDeps,
  storeLiveAt,
  streamCarries,
} from "./relayStoreCoverage.ts";
import { UserStreamSession } from "./relayUserStream.ts";
import { UserStreamLive } from "./relayUserStreamLive.ts";

// The user stream in live/gated over a real room bundle, with the store and
// the relay faked (docs/message-sync.md#user-stream).
const fixture = JSON.parse(
  readFileSync(
    new URL("../../../../test-fixtures/user-stream.json", import.meta.url),
    "utf8",
  ),
);

const L = 2_000_000;
const RELAY = "f".repeat(64);
const flush = () => new Promise((resolve) => setImmediate(resolve));

const recorded = [];
let restore = () => {};
beforeEach(() => {
  recorded.length = 0;
  restore = setStoreCoverageDeps({
    nowSecs: () => L,
    extend: async (roomIds, from, to) => {
      recorded.push([[...roomIds].sort(), from, to]);
    },
  });
});
afterEach(() => restore());

function part1(mode, rooms, createdAt = L) {
  return {
    id: "p1",
    kind: 24220,
    pubkey: RELAY,
    created_at: createdAt,
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
    }),
  };
}

const roomEvent = (id, room, createdAt = L, kind = 9) => ({
  id,
  kind,
  pubkey: "a".repeat(64),
  created_at: createdAt,
  tags: [["h", room]],
  content: "",
});

/** Appends `[from, to]` to each room's spans (`resume` as a store). */
function addSpans(resume, roomIds, from, to) {
  for (const room of roomIds) {
    const spans = resume[room];
    const before =
      spans === undefined
        ? []
        : Array.isArray(spans)
          ? spans
          : [[spans, spans]];
    resume[room] = [...before, [from, to]];
  }
}

/**
 * A session with live hooks over a bundle whose chunks are recorded.
 * `pages(request)` answers the relay's history walks.
 */
function harness({
  rows = {},
  epoch = "e1",
  resume = {},
  pages = () => [],
  state = {},
  segments = null,
  storedBefore = [],
  vouched = true,
  visibleRoom = null,
  rateLimit = null,
  persist = false,
} = {}) {
  const open = new Map();
  const bundle = new RoomLiveBundle(
    async (filter, onEvent) => {
      const key = filter["#h"].join(",");
      open.set(key, onEvent);
      return async () => {
        if (open.get(key) === onEvent) open.delete(key);
      };
    },
    {
      setTimeout: (fn, ms) => setTimeout(fn, ms),
      clearTimeout: (id) => clearTimeout(id),
    },
  );
  const calls = {
    epoch: [],
    extend: [],
    clear: [],
    pages: [],
    saved: {},
    before: [],
    fresh: [],
  };
  // A number is a span ending there; a list holds `[lo, hi]` spans, of
  // which only those beginning before `before` count (as in the store).
  const pointsBefore = (roomIds, before) =>
    Object.fromEntries(
      roomIds.flatMap((room) => {
        const spans = resume[room];
        if (spans === undefined) return [];
        if (!Array.isArray(spans)) return [[room, spans]];
        const his = spans.filter(([lo]) => lo < before).map(([, hi]) => hi);
        return his.length === 0 ? [] : [[room, Math.max(...his)]];
      }),
    );
  const store = {
    epoch: async (value) => {
      calls.epoch.push(value);
      return true;
    },
    seqs: async (roomIds, before) => {
      calls.before.push(before);
      if (!vouched) return { vouched, epoch: null, rows: {}, resume: {} };
      return {
        vouched,
        epoch,
        rows: Object.fromEntries(
          roomIds
            .filter((room) => rows[room])
            .map((room) => [room, rows[room]]),
        ),
        resume: pointsBefore(roomIds, before),
      };
    },
    clearRows: async (roomIds) => {
      calls.clear.push(roomIds);
    },
    scope: async () => "scope-1",
    resumePoints: async (roomIds, before) => pointsBefore(roomIds, before),
    // By default each room's spans are its resume spans.
    segments: async (roomIds) =>
      segments === "none"
        ? null
        : (segments ??
          Object.fromEntries(
            roomIds.flatMap((room) => {
              const spans = resume[room];
              if (spans === undefined) return [];
              return [[room, Array.isArray(spans) ? spans : [[spans, spans]]]];
            }),
          )),
    // Rows stored before the connection started: `storedBefore`.
    freshIds: async (ids) => {
      calls.fresh.push(ids);
      return ids.filter((id) => !storedBefore.includes(id));
    },
    extend: async (roomIds, from, to, scope, seq) => {
      calls.extend.push({ roomIds, from, to, scope, seq });
      if (persist) addSpans(resume, roomIds, from, to);
    },
    getState: async (key) => state[key] ?? null,
    setState: async (key, value) => {
      calls.saved[key] = value;
    },
  };
  const host = {
    now: L * 1000,
    sent: [],
    closedSockets: [],
    logs: [],
    refreshes: 0,
    timers: [],
  };
  const session = new UserStreamSession({
    generation: () => 1,
    send: async (payload) => {
      host.sent.push(payload);
    },
    closeSocket: (generation) => host.closedSockets.push(generation),
    chunkRooms: () => bundle.chunkRooms(),
    roomOf: (event) => bundle.streamRoomOf(event),
    nowMs: () => host.now,
    setTimeout: () => 0,
    clearTimeout: () => {},
    log: (line) => host.logs.push(line),
    enabled: () => true,
    random: () => 0.5,
    loadState: store.getState,
    saveState: (key, value) => {
      calls.saved[key] = value;
    },
  });
  const live = new UserStreamLive({
    nowSecs: () => Math.floor(host.now / 1000),
    store,
    bundle,
    roomOf: (event) => bundle.streamRoomOf(event),
    requestPage: async (request) => {
      calls.pages.push(request);
      return pages(request);
    },
    report: () => session.report,
    listRefresh: () => {
      host.refreshes += 1;
    },
    nowMs: () => host.now,
    setTimer: (ms, run) => host.timers.push({ at: host.now + ms, run }),
    log: (line) => host.logs.push(line),
    random: () => 0,
    visibleRoom: () => visibleRoom,
    rateLimited: () => rateLimit?.limited() ?? false,
    waitForRateLimit: () => rateLimit?.wait() ?? Promise.resolve(),
  });
  session.liveHooks = live;
  const start = async (mode, snapshotRooms, connectAt = L) => {
    host.now = connectAt * 1000;
    await session.connected(1);
    await session.requested;
    host.now = L * 1000;
    const subId = host.sent.at(-1)[1];
    session.event(subId, part1(mode, snapshotRooms));
    session.eose(subId);
    await live.settled;
    await flush();
    return subId;
  };
  return { bundle, open, calls, host, session, live, start };
}

test("live: after the EOSE the snapshot rooms leave their chunks; extras and the rest stay", async () => {
  const h = harness({ resume: { "room-a": L - 100, "room-b": L - 100 } });
  h.bundle.setMemberRooms(["room-a", "room-b"]);
  void h.bundle.listenRoom("room-x", () => {});
  await flush();
  assert.deepEqual([...h.open.keys()].sort(), ["room-a,room-b", "room-x"]);
  const subId = await h.start("live", { "room-a": 1, "room-b": 2 });
  assert.deepEqual([...h.open.keys()].sort(), ["room-x"]);
  // A member room joining later gets a chunk of its own.
  h.bundle.setMemberRooms(["room-a", "room-b", "room-c"]);
  await flush();
  assert.deepEqual([...h.open.keys()].sort(), ["room-c", "room-x"]);
  assert.equal(streamCarries("room-a"), true);
  assert.equal(storeLiveAt("room-a"), L);
  assert.deepEqual(recorded[0], [["room-a", "room-b"], L, L]);
  // The stream delivers once by id.
  const seen = [];
  h.bundle.listenAllRooms((event) => seen.push(event.id));
  h.session.event(subId, roomEvent("e1", "room-a"));
  h.session.event(subId, roomEvent("e1", "room-a"));
  assert.deepEqual(seen, ["e1"]);
  // The socket ends: every carried room is on chunks again.
  h.session.socketEnded();
  await flush();
  assert.ok(h.open.has("room-a,room-b,room-c"));
  assert.equal(streamCarries("room-a"), false);
});

test("live: the catch-up walks covered rooms by resume point and writes (S, L) with the span", async () => {
  const h = harness({
    rows: { "room-a": [1, 1_999_000] },
    resume: { "room-a": 1_999_000, "room-b": 1_998_000 },
  });
  await h.start("live", { "room-a": 1, "room-b": 2, "room-u": 3 });
  assert.deepEqual(h.calls.epoch, ["e1"]);
  assert.equal(h.calls.extend.length, 1);
  const [write] = h.calls.extend;
  assert.deepEqual(write.roomIds, ["room-b", "room-a"]);
  assert.equal(write.from, 1_998_000 - 1865);
  assert.equal(write.to, L);
  assert.deepEqual(write.seq, {
    epoch: "e1",
    seqs: { "room-b": 2, "room-a": 1 },
  });
  assert.equal(h.calls.pages[0].since, 1_998_000 - 1865);
  const counters = h.session.report.counters;
  assert.equal(counters.predictedSkips, 1);
  assert.equal(counters.verifiedSkips, 1);
  assert.equal(counters.falseSkip ?? 0, 0);
});

test("live: a predicted skip with a new row counts falseSkip", async () => {
  const h = harness({
    rows: { "room-a": [1, 1_999_000] },
    resume: { "room-a": 1_999_000 },
    segments: { "room-a": [[1_990_000, 1_999_000]] },
    pages: () => [roomEvent("missed", "room-a", 1_999_500)],
  });
  await h.start("live", { "room-a": 1 });
  assert.equal(h.session.report.counters.falseSkip, 1);
  assert.equal(h.session.report.counters.verifiedSkips ?? 0, 0);
});

test("gated: equal rooms are not walked, then audited per room", async () => {
  const h = harness({
    rows: { "room-a": [1, 1_999_000] },
    resume: { "room-a": 1_999_000, "room-b": 1_998_000 },
  });
  await h.start("gated", { "room-a": 1, "room-b": 2 });
  assert.deepEqual(
    h.calls.extend.map((call) => call.roomIds),
    [["room-b"], ["room-a"]],
    "the walk, then the audit",
  );
  assert.equal(h.session.report.counters.skipAudited, 1);
  assert.equal(h.session.report.counters.skipAuditFailed ?? 0, 0);
});

test("gated: a failed audit batch counts its rooms as skipAuditFailed", async () => {
  const h = harness({
    rows: { "room-a": [1, 1_999_000] },
    resume: { "room-a": 1_999_000 },
    pages: (request) => {
      if (request.channelIds.includes("room-a")) throw new Error("down");
      return [];
    },
  });
  await h.start("gated", { "room-a": 1 });
  assert.equal(h.session.report.counters.skipAuditFailed, 1);
  assert.equal(h.calls.extend.length, 0);
});

test("gated: rows of another epoch are none, so nothing is skipped", async () => {
  const h = harness({
    epoch: "e0",
    rows: { "room-a": [1, 1_999_000] },
    resume: { "room-a": 1_999_000 },
  });
  await h.start("gated", { "room-a": 1 });
  assert.equal(h.session.report.counters.predictedSkips ?? 0, 0);
  assert.deepEqual(h.calls.extend[0].roomIds, ["room-a"]);
});

test("a stored seq ahead of the snapshot is cleared and the room walked", async () => {
  const h = harness({
    rows: { "room-a": [9, 1_999_000] },
    resume: { "room-a": 1_999_000 },
  });
  await h.start("gated", { "room-a": 1 });
  assert.deepEqual(h.calls.clear, [["room-a"]]);
  assert.deepEqual(h.calls.extend[0].seq.seqs, { "room-a": 1 });
});

test("mid-connection CLOSED: chunks for every carried room, catch-up from resume points, listener repair", async () => {
  let repairRows = [];
  const h = harness({
    resume: { "room-a": 1_999_000, "room-b": 1_999_000 },
    pages: (request) => (request.since === L + 30 - 1865 ? repairRows : []),
  });
  h.bundle.setMemberRooms(["room-a", "room-b"]);
  await flush();
  const subId = await h.start("live", { "room-a": 1, "room-b": 2 });
  assert.deepEqual([...h.open.keys()], []);
  h.session.event(subId, roomEvent("f1", "room-a", L + 30));
  h.host.now += 120_000;
  h.calls.extend.length = 0;
  const shown = [];
  h.bundle.listenAllRooms((event) => shown.push(event.id));
  repairRows = [
    roomEvent("r1", "room-b", L + 60),
    roomEvent("old", "room-b", L - 100),
  ];
  h.session.closed(subId, "error: user-stream closed");
  await flush();
  await flush();
  assert.ok(h.open.has("room-a,room-b"), "chunks reopen at once");
  assert.equal(streamCarries("room-a"), false);
  const closedAt = L + 120;
  assert.deepEqual(
    h.calls.extend.map((call) => [call.roomIds, call.from, call.to, call.seq]),
    [[["room-a", "room-b"], 1_999_000 - 1865, closedAt, undefined]],
  );
  assert.deepEqual(
    shown,
    ["r1"],
    "rows before the live start minus 5 s stay out",
  );
  assert.equal(h.session.report.counters.fallback_closed, 1);
});

test("auth-required after the stream EOSE in live: the mid-connection fallback, no retry", async () => {
  const h = harness({ resume: { "room-a": 1_999_000 } });
  h.bundle.setMemberRooms(["room-a"]);
  await flush();
  const subId = await h.start("live", { "room-a": 1 });
  const sent = h.host.sent.length;
  h.host.now += 60_000;
  h.calls.extend.length = 0;
  h.session.closed(subId, "auth-required: authenticate first");
  await flush();
  await flush();
  assert.ok(h.open.has("room-a"), "chunks reopen at once");
  h.session.authOk(1);
  assert.equal(h.host.sent.length, sent, "the REQ is not sent again");
  assert.deepEqual(
    h.calls.extend.map((call) => [call.from, call.to]),
    [[1_999_000 - 1865, L + 60]],
    "catch-up up to the CLOSED time",
  );
  assert.equal(h.session.report.counters.fallback_auth_required, 1);
});

test("an overflow in live waits for the socket: no chunk opens until it ends", async () => {
  const h = harness({ resume: { "room-a": L - 100 } });
  h.bundle.setMemberRooms(["room-a"]);
  await flush();
  const subId = await h.start("live", { "room-a": 1 });
  h.session.closed(subId, "error: user-stream overflow");
  await flush();
  assert.deepEqual([...h.open.keys()], []);
  assert.equal(streamCarries("room-a"), false, "its span stops at the CLOSED");
  assert.equal(h.bundle.isRoomLive("room-a"), false, "not live while it waits");
  h.session.socketEnded();
  await flush();
  assert.ok(h.open.has("room-a"));
});

test("membership: a 44100 after the EOSE carries the room; a 44101 hides later frames", async () => {
  const h = harness({ resume: { "room-a": L - 100 } });
  h.bundle.setMemberRooms(["room-a", "room-n"]);
  await flush();
  const subId = await h.start("live", { "room-a": 1 });
  recorded.length = 0;
  h.host.now += 10_000;
  h.session.membershipNotice({
    ...roomEvent("j1", "room-n", L + 5, 44100),
  });
  assert.equal(streamCarries("room-n"), true);
  await flush();
  assert.deepEqual(recorded, [[["room-n"], L + 10, L + 10]]);
  assert.equal(h.open.has("room-n"), false, "its chunk is replaced");
  const shown = [];
  h.bundle.listenAllRooms((event) => shown.push(event.id));
  h.session.membershipNotice(roomEvent("l1", "room-a", L + 20, 44101));
  h.session.event(subId, roomEvent("same", "room-a", L + 20));
  h.session.event(subId, roomEvent("after", "room-a", L + 21));
  assert.deepEqual(shown, ["same"]);
});

test("routing: an unknown room asks for a list refresh; an open archived room reaches its listener", async () => {
  const h = harness();
  h.bundle.setMemberRooms(["room-a"]);
  const opened = [];
  void h.bundle.listenRoom("room-arch", (event) => opened.push(event.id));
  const all = [];
  h.bundle.listenAllRooms((event) => all.push(event.id));
  await flush();
  const subId = await h.start("live", { "room-a": 1, "room-arch": 1 });
  h.session.event(subId, roomEvent("u1", "room-unknown"));
  h.session.event(subId, roomEvent("ar1", "room-arch"));
  assert.equal(h.host.refreshes, 1);
  // S6: at most once per 10 s per room, not per frame.
  h.host.now = L * 1000 + 2_000;
  h.session.event(subId, roomEvent("u2", "room-unknown"));
  h.session.event(subId, roomEvent("ar2", "room-arch"));
  assert.equal(h.host.refreshes, 1);
  assert.deepEqual(opened, ["ar1", "ar2"]);
  assert.deepEqual(all, []);
  // Another unknown room within 5 s of the last refresh waits out the gap.
  h.session.event(subId, roomEvent("v1", "room-other"));
  h.session.event(subId, roomEvent("v2", "room-third"));
  assert.equal(h.host.refreshes, 1);
  assert.deepEqual(
    h.host.timers.map((timer) => timer.at),
    [L * 1000 + 5_000],
  );
  h.host.now = L * 1000 + 5_000;
  h.host.timers.shift().run();
  assert.equal(h.host.refreshes, 2);
  h.host.now = (L + 9) * 1000;
  h.session.event(subId, roomEvent("u3", "room-unknown"));
  assert.equal(h.host.refreshes, 2);
  // 10 s after its last ask the first room may ask again.
  h.host.now = (L + 11) * 1000;
  h.session.event(subId, roomEvent("u4", "room-unknown"));
  assert.equal(h.host.refreshes, 3);
  assert.deepEqual(h.host.timers, []);
});

test("the stream's span grows at most once a minute and stops at its CLOSED", async () => {
  const h = harness({ resume: { "room-a": L - 100 } });
  const subId = await h.start("live", { "room-a": 1 });
  recorded.length = 0;
  for (const at of [30, 61, 90, 125]) {
    h.host.now = (L + at) * 1000;
    h.session.event(subId, roomEvent(`s${at}`, "room-a", L + at));
  }
  h.session.closed(subId, "error: user-stream closed");
  h.host.now = (L + 300) * 1000;
  h.session.event(subId, roomEvent("late", "room-a", L + 300));
  await flush();
  assert.deepEqual(recorded, [
    [["room-a"], L, L + 61],
    [["room-a"], L, L + 125],
  ]);
  assert.ok(h.calls.saved.stream_prev_live, "kept at every extension");
});

test("liveMissed: new rows of the previous live set inside its window", async () => {
  const prev = {
    liveRooms: ["room-a"],
    eoseAt: L - 5000,
    lastFrameAt: L - 1000,
  };
  const h = harness({
    resume: { "room-a": L - 1500 },
    state: { stream_prev_live: JSON.stringify(prev) },
    segments: { "room-a": [[L - 6000, L - 1500]] },
    pages: () => [
      roomEvent("missed", "room-a", L - 2000),
      roomEvent("ok-late", "room-a", L - 500),
      { ...roomEvent("relay9005", "room-a", L - 2000, 9005), pubkey: RELAY },
    ],
  });
  await h.start("live", { "room-a": 1 });
  assert.equal(h.session.report.counters.liveRoomsChecked, 1);
  assert.equal(h.session.report.counters.liveMissed, 1);
  const saved = JSON.parse(h.calls.saved.stream_prev_live);
  assert.deepEqual(saved.liveRooms, ["room-a"]);
  assert.equal(saved.eoseAt, L);
});

test("backoff and counters come from the store and are kept on change", async () => {
  const h = harness({
    state: {
      stream_backoff: JSON.stringify({ closes: [], until: L + 100 }),
      stream_counters: JSON.stringify({ seq: 4, counters: { reconnects: 7 } }),
    },
  });
  await h.session.connected(1);
  await h.session.requested;
  assert.deepEqual(h.host.sent, [], "still backed off after a restart");
  assert.equal(h.session.report.counters.reconnects, 7);
  h.session.networkChange();
  assert.deepEqual(JSON.parse(h.calls.saved.stream_backoff), {
    closes: [],
    until: 0,
  });
  await h.session.connected(1);
  assert.equal(h.host.sent.length, 1);
});

test("B1: a chunk EOSE span of this connection is neither the resume point nor the new-row base", async () => {
  const h = harness({
    rows: { "room-a": [1, L - 7200] },
    resume: {
      "room-a": [
        [L - 30_000, L - 7200],
        [L - 2, L - 2],
      ],
    },
    segments: {
      "room-a": [
        [L - 30_000, L - 7200],
        [L - 2, L - 2],
      ],
    },
    pages: () => [roomEvent("gap", "room-a", L - 3600)],
  });
  await h.start("live", { "room-a": 1 }, L - 10);
  assert.deepEqual(h.calls.before, [L - 10]);
  assert.equal(h.calls.extend[0].from, L - 7200 - 1865);
  assert.equal(h.calls.pages[0].since, L - 7200 - 1865);
  assert.equal(h.session.report.counters.falseSkip, 1, "the gap row is new");
});

test("B2: overflow twice, socket end and a backed-off connect leave every room on chunks", async () => {
  const h = harness({ resume: { "room-a": L - 100, "room-b": L - 100 } });
  h.bundle.setMemberRooms(["room-a", "room-b"]);
  await flush();
  for (let round = 0; round < 2; round += 1) {
    const subId = await h.start("live", { "room-a": 1, "room-b": 2 });
    assert.deepEqual([...h.open.keys()], []);
    h.session.closed(subId, "error: user-stream overflow");
    await flush();
    h.session.socketEnded();
    await flush();
    assert.ok(h.open.has("room-a,room-b"));
  }
  const sent = h.host.sent.length;
  await h.session.connected(1);
  assert.equal(h.host.sent.length, sent, "backed off");
  await flush();
  assert.ok(h.open.has("room-a,room-b"), "chunks carry every member room");
});

test("S1: a restore that finishes after an account switch is dropped", async () => {
  let release;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  const h = harness();
  const loads = [];
  h.session.host.loadState = async (key) => {
    loads.push(key);
    if (loads.length <= 2) {
      await gate;
      return key === "stream_backoff"
        ? JSON.stringify({ closes: [], until: L + 3600 })
        : JSON.stringify({ seq: 9, counters: { reconnects: 5 } });
    }
    return null;
  };
  await h.session.connected(1);
  const first = h.session.requested;
  h.session.scopeChanged();
  await h.session.connected(1);
  await h.session.requested;
  assert.equal(h.host.sent.length, 1, "account B's REQ is sent");
  release();
  await first;
  assert.equal(h.host.sent.length, 1, "account A's late restore sends nothing");
  assert.equal(h.session.report.counters.reconnects ?? 0, 0);
  h.session.networkChange();
  assert.deepEqual(JSON.parse(h.calls.saved.stream_backoff).until, 0);
});

/** A second connection at `at` (s) after a socket end, through its EOSE. */
async function reconnect(h, mode, snapshotRooms, at) {
  h.session.socketEnded();
  await flush();
  h.host.now = at * 1000;
  await h.session.connected(1);
  await h.session.requested;
  const subId = h.host.sent.at(-1)[1];
  h.session.event(subId, part1(mode, snapshotRooms, at));
  h.session.eose(subId);
  await h.live.settled;
  await flush();
  return subId;
}

test("B1: after a quiet connection the reconnect's listener repair brings the gap to every listener", async () => {
  const gap = roomEvent("gap", "room-a", L + 300);
  const h = harness({
    resume: { "room-a": L - 100 },
    pages: (request) => (request.since === L - 1865 ? [gap] : []),
  });
  h.bundle.setMemberRooms(["room-a"]);
  const all = [];
  const opened = [];
  h.bundle.listenAllRooms((event) => all.push(event.id));
  void h.bundle.listenRoom("room-a", (event) => opened.push(event.id));
  await flush();
  // Connection 1 sees no stream frame at all.
  await h.start("live", { "room-a": 1 });
  await reconnect(h, "live", { "room-a": 2 }, L + 600);
  assert.ok(
    h.calls.pages.some(
      (request) =>
        request.since === L - 1865 && request.channelIds.includes("room-a"),
    ),
    "repair from the last EOSE minus the lookback",
  );
  assert.deepEqual(all, ["gap"]);
  assert.deepEqual(opened, ["gap"]);
});

test("S2: rooms with no usable resume point keep their chunk and its repair; only covered fresh rooms are handed", async () => {
  const h = harness({
    resume: { "room-a": L - 100, "room-s": L - 8 * 24 * 3600 },
  });
  h.bundle.setMemberRooms(["room-a", "room-s", "room-u"]);
  await flush();
  const chunk = h.open.get("room-a,room-s,room-u");
  assert.ok(chunk);
  const all = [];
  h.bundle.listenAllRooms((event) => all.push(event.id));
  await h.start("live", { "room-a": 1, "room-s": 1, "room-u": 1 });
  assert.equal(
    h.open.get("room-a,room-s,room-u"),
    chunk,
    "the same chunk stays: its reconnect repair goes on",
  );
  assert.equal(streamCarries("room-a"), true);
  assert.equal(streamCarries("room-u"), false);
  assert.equal(streamCarries("room-s"), false);
  assert.equal(h.session.report.counters.handoffKept, 2);
  // The chunk's repair still delivers the uncovered room's gap.
  chunk(roomEvent("gapU", "room-u", L - 50));
  assert.deepEqual(all, ["gapU"]);
  // The stream's own walks never ask for the kept rooms.
  for (const request of h.calls.pages) {
    assert.deepEqual(request.channelIds, ["room-a"]);
  }
  // At the socket end and the next connect, the chunks are partitioned again.
  h.session.socketEnded();
  await flush();
  assert.ok(h.open.has("room-a,room-s,room-u"));
});

test("S2: nothing is handed when the store cannot vouch (a write loss pending)", async () => {
  const h = harness({ resume: { "room-a": L - 100 }, vouched: false });
  h.bundle.setMemberRooms(["room-a"]);
  await flush();
  await h.start("live", { "room-a": 1 });
  assert.ok(h.open.has("room-a"));
  assert.equal(streamCarries("room-a"), false);
  assert.deepEqual(h.calls.pages, []);
});

test("S1: a gap row the chunk repair stored before the stream EOSE still counts as new", async () => {
  const h = harness({
    rows: { "room-a": [1, L - 7200] },
    resume: { "room-a": L - 7200 },
    segments: { "room-a": [[L - 30_000, L - 7200]] },
    storedBefore: ["old"],
    pages: () => [
      roomEvent("gap", "room-a", L - 3600),
      roomEvent("old", "room-a", L - 3500),
      roomEvent("live1", "room-a", L - 3400),
      roomEvent("before-base", "room-a", L - 40_000),
    ],
  });
  h.bundle.setMemberRooms(["room-a"]);
  await flush();
  const chunk = h.open.get("room-a");
  await h.session.connected(1);
  await h.session.requested;
  // Before the stream EOSE: the chunk repair delivers (and stores) the gap
  // row; a chunk frame arrives live on the socket.
  chunk(roomEvent("gap", "room-a", L - 3600));
  h.live.noteLive("live1");
  const subId = h.host.sent.at(-1)[1];
  h.session.event(subId, part1("live", { "room-a": 1 }));
  h.session.eose(subId);
  await h.live.settled;
  await flush();
  assert.equal(h.session.report.counters.falseSkip, 1, "the gap row is new");
  assert.equal(h.session.report.counters.verifiedSkips ?? 0, 0);
  // S9: only candidate rows are asked about (not live, inside the window).
  assert.deepEqual(h.calls.fresh, [["gap", "old"]]);
});

test("S1: a walk whose rows cannot be told verifies nothing", async () => {
  const h = harness({
    rows: { "room-a": [1, L - 7200] },
    resume: { "room-a": L - 7200 },
    segments: "none",
    pages: () => [],
  });
  await h.start("live", { "room-a": 1 });
  assert.equal(h.session.report.counters.predictedSkips, 1);
  assert.equal(h.session.report.counters.verifiedSkips ?? 0, 0);
  assert.equal(h.session.report.counters.newRowUncounted, 1);
});

test("S4: the listener repair does not wait for the catch-up, and the room on screen goes first", async () => {
  let release;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  let hold = false;
  const h = harness({
    resume: { "room-a": L - 100, "room-v": L - 100 },
    visibleRoom: "room-v",
    pages: async (request) => {
      if (hold && request.since === L - 100 - 1865) await gate;
      return [];
    },
  });
  h.bundle.setMemberRooms(["room-a", "room-v"]);
  await flush();
  await h.start("live", { "room-a": 1, "room-v": 1 });
  h.session.socketEnded();
  await flush();
  h.host.now = (L + 600) * 1000;
  await h.session.connected(1);
  await h.session.requested;
  const subId = h.host.sent.at(-1)[1];
  h.calls.pages.length = 0;
  hold = true;
  h.session.event(subId, part1("live", { "room-a": 1, "room-v": 1 }));
  h.session.eose(subId);
  for (let i = 0; i < 5; i += 1) await flush();
  const repairs = h.calls.pages.filter((request) => request.since === L - 1865);
  assert.deepEqual(
    repairs.map((request) => request.channelIds),
    [["room-v"], ["room-a"]],
    "repair ran while the catch-up page was still out",
  );
  release();
  await h.live.settled;
});

test("S5: walks start no page after their connection ends, and wait for the rate limit", async () => {
  let limited = true;
  let lift;
  const lifted = new Promise((resolve) => {
    lift = resolve;
  });
  const resume = {};
  const snapshot = {};
  for (let i = 0; i < 130; i += 1) {
    const room = `room-${String(i).padStart(3, "0")}`;
    resume[room] = L - 100 - i;
    snapshot[room] = 1;
  }
  const h = harness({
    resume,
    rateLimit: { limited: () => limited, wait: () => lifted },
  });
  await h.session.connected(1);
  await h.session.requested;
  const subId = h.host.sent.at(-1)[1];
  h.session.event(subId, part1("live", snapshot));
  h.session.eose(subId);
  for (let i = 0; i < 5; i += 1) await flush();
  assert.deepEqual(h.calls.pages, [], "no page while rate limited");
  h.session.socketEnded();
  limited = false;
  lift();
  await h.live.settled;
  await flush();
  assert.deepEqual(h.calls.pages, [], "the ended connection walks nothing");
});

test("S5: a connection's walks start only after the earlier connection's walks ended", async () => {
  let release;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  let hold = true;
  const h = harness({
    resume: { "room-a": L - 100 },
    pages: async () => {
      if (hold) {
        hold = false;
        await gate;
      }
      return [];
    },
  });
  await h.session.connected(1);
  await h.session.requested;
  let subId = h.host.sent.at(-1)[1];
  h.session.event(subId, part1("live", { "room-a": 1 }));
  h.session.eose(subId);
  for (let i = 0; i < 5; i += 1) await flush();
  assert.equal(h.calls.pages.length, 1, "the first catch-up page is out");
  h.session.socketEnded();
  await flush();
  h.host.now = (L + 600) * 1000;
  await h.session.connected(1);
  await h.session.requested;
  subId = h.host.sent.at(-1)[1];
  h.session.event(subId, part1("live", { "room-a": 1 }));
  h.session.eose(subId);
  for (let i = 0; i < 5; i += 1) await flush();
  assert.equal(h.calls.pages.length, 1, "no page while the earlier one is out");
  release();
  await h.live.settled;
  await flush();
  assert.ok(h.calls.pages.length > 1, "the later catch-up walks after it");
});

test("S7: shadow counts predicted skips and verifies them through a later chunk catch-up", async () => {
  const seqWrites = [];
  const restorePages = setStoreCoverageDeps({
    nowSecs: () => L + 5,
    resumePoints: async () => ({ "room-a": L - 7200, "room-b": L - 7200 }),
    requestPage: async () => [roomEvent("new-b", "room-b", L - 3600)],
    extend: async (roomIds, _from, _to, _scope, seq) => {
      seqWrites.push({ roomIds: [...roomIds].sort(), seq });
    },
  });
  try {
    const h = harness({
      rows: { "room-a": [1, L - 7200], "room-b": [2, L - 7200] },
      resume: { "room-a": L - 7200, "room-b": L - 7200 },
      segments: {
        "room-a": [[L - 30_000, L - 7200]],
        "room-b": [[L - 30_000, L - 7200]],
      },
    });
    await h.start("shadow", { "room-a": 1, "room-b": 2, "room-c": 1 });
    assert.deepEqual(h.calls.epoch, ["e1"], "shadow queues the epoch too");
    assert.equal(h.session.report.counters.predictedSkips, 2);
    assert.deepEqual(h.calls.pages, [], "shadow walks nothing itself");
    // A chunk EOSE catch-up after the stream EOSE on this connection.
    assert.equal(
      await catchUpFromStore(["room-a", "room-b"], L + 5, Promise.resolve("s")),
      true,
    );
    await flush();
    await flush();
    assert.deepEqual(seqWrites, [
      {
        roomIds: ["room-a", "room-b"],
        seq: { epoch: "e1", seqs: { "room-a": 1, "room-b": 2 } },
      },
    ]);
    assert.equal(h.session.report.counters.verifiedSkips, 1);
    assert.equal(h.session.report.counters.falseSkip, 1);
    // Each room once; nothing after the socket ends.
    await catchUpFromStore(["room-a", "room-b"], L + 6, Promise.resolve("s"));
    await flush();
    await flush();
    assert.equal(h.session.report.counters.verifiedSkips, 1);
    h.session.socketEnded();
    seqWrites.length = 0;
    await catchUpFromStore(["room-a"], L + 7, Promise.resolve("s"));
    await flush();
    assert.equal(seqWrites[0].seq, undefined, "no rule-W rows after it");
  } finally {
    restorePages();
  }
});

test("S8: the first connect does not wait for the store restore; the REQ follows it", async () => {
  let release;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  const h = harness();
  h.session.host.loadState = async () => {
    await gate;
    return null;
  };
  await h.session.connected(1);
  assert.deepEqual(h.host.sent, [], "returned before the restore landed");
  release();
  await h.session.requested;
  assert.equal(h.host.sent.length, 1);
  assert.equal(h.host.sent[0][0], "REQ");
});

test("B1: a catch-up cut short by a drop writes no stream span; the next connection resumes from the real gap start", async () => {
  // 2 h offline, 200 covered rooms; the socket drops during batch 1.
  const resume = {};
  const snapshot = {};
  const rooms = [];
  for (let i = 0; i < 200; i += 1) {
    const room = `room-${String(i).padStart(3, "0")}`;
    rooms.push(room);
    resume[room] = [[L - 30_000, L - 7200]];
    snapshot[room] = 1;
  }
  let dropped;
  const drop = new Promise((resolve) => {
    dropped = resolve;
  });
  let first = true;
  const h = harness({
    resume,
    persist: true,
    pages: async () => {
      if (first) {
        first = false;
        await drop;
        throw new Error("socket closed");
      }
      return [];
    },
  });
  const restoreSpans = setStoreCoverageDeps({
    nowSecs: () => Math.floor(h.host.now / 1000),
    extend: async (roomIds, from, to) => {
      recorded.push([[...roomIds].sort(), from, to]);
      addSpans(resume, roomIds, from, to);
    },
  });
  try {
    await h.session.connected(1);
    await h.session.requested;
    let subId = h.host.sent.at(-1)[1];
    h.session.event(subId, part1("live", snapshot));
    h.session.eose(subId);
    for (let i = 0; i < 5; i += 1) await flush();
    assert.equal(h.calls.pages.length, 1, "batch 1 is in flight");
    assert.equal(h.calls.pages[0].since, L - 7200 - 1865);
    assert.equal(h.calls.pages[0].channelIds.length, 128);
    // A frame a minute on extends no span: no batch has completed.
    h.host.now = (L + 61) * 1000;
    h.session.event(subId, roomEvent("f1", "room-000", L + 61));
    await flush();
    assert.deepEqual(recorded, [], "no stream span before a batch completes");
    assert.equal(streamCarries("room-000"), true, "carried, in memory");
    h.session.socketEnded();
    dropped();
    await h.live.settled;
    await flush();
    assert.deepEqual(recorded, [], "the cut-short catch-up wrote no span");
    assert.equal(h.calls.extend.length, 0);
    // Connection 2, 10 min later.
    h.calls.pages.length = 0;
    h.host.now = (L + 600) * 1000;
    await h.session.connected(1);
    await h.session.requested;
    subId = h.host.sent.at(-1)[1];
    h.session.event(subId, part1("live", snapshot, L + 600));
    h.session.eose(subId);
    await h.live.settled;
    await flush();
    const catchUp = h.calls.pages.filter(
      (request) => request.since === L - 7200 - 1865,
    );
    assert.deepEqual(
      catchUp.map((request) => request.channelIds.length),
      [128, 72],
      "every room walks from the real gap start, not from L",
    );
    assert.deepEqual(
      h.calls.extend.map((call) => [call.roomIds.length, call.from, call.to]),
      [
        [128, L - 7200 - 1865, L + 600],
        [72, L - 7200 - 1865, L + 600],
      ],
    );
    // Each completed batch then gets its stream span.
    assert.deepEqual(
      recorded.map(([ids, from, to]) => [ids.length, from, to]),
      [
        [128, L + 600, L + 600],
        [72, L + 600, L + 600],
      ],
    );
    assert.deepEqual(recorded.flatMap(([ids]) => ids).sort(), rooms);
  } finally {
    restoreSpans();
  }
});

test("gated: a skipped equal room gets its stream span at once; a walked room after its batch", async () => {
  let release;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  const h = harness({
    rows: { "room-a": [1, 1_999_000] },
    resume: { "room-a": 1_999_000, "room-b": 1_998_000 },
    pages: async (request) => {
      if (request.channelIds.includes("room-b")) await gate;
      return [];
    },
  });
  await h.session.connected(1);
  await h.session.requested;
  const subId = h.host.sent.at(-1)[1];
  h.session.event(subId, part1("gated", { "room-a": 1, "room-b": 2 }));
  h.session.eose(subId);
  for (let i = 0; i < 5; i += 1) await flush();
  assert.deepEqual(recorded, [[["room-a"], L, L]]);
  release();
  await h.live.settled;
  await flush();
  assert.deepEqual(recorded, [
    [["room-a"], L, L],
    [["room-b"], L, L],
  ]);
});

test("S-D1: after a chunk-only connection the next live connection runs the listener repair from its start", async () => {
  const gap = roomEvent("gap", "room-a", L + 300);
  const h = harness({
    resume: { "room-a": L - 100 },
    pages: (request) => (request.since === L - 1865 ? [gap] : []),
  });
  h.bundle.setMemberRooms(["room-a"]);
  const all = [];
  h.bundle.listenAllRooms((event) => all.push(event.id));
  await flush();
  // Connection 1 at L: the relay has the stream off; chunks carry the room.
  await h.session.connected(1);
  await h.session.requested;
  h.session.closed(h.host.sent.at(-1)[1], "unsupported: user-stream off");
  await reconnect(h, "live", { "room-a": 2 }, L + 600);
  assert.ok(
    h.calls.pages.some(
      (request) =>
        request.since === L - 1865 && request.channelIds.includes("room-a"),
    ),
    "repair from the chunk-only connection's start minus the lookback",
  );
  assert.deepEqual(all, ["gap"]);
});

test("S-D2: a chunk carrying a room outside the snapshot stays whole after the hand-off", async () => {
  const h = harness({ resume: { "room-a": L - 100, "room-b": L - 100 } });
  h.bundle.setMemberRooms(["room-a", "room-b", "room-x"]);
  await flush();
  const chunk = h.open.get("room-a,room-b,room-x");
  assert.ok(chunk);
  await h.start("live", { "room-a": 1, "room-b": 1 });
  assert.equal(
    h.open.get("room-a,room-b,room-x"),
    chunk,
    "not re-partitioned: its repair goes on",
  );
  assert.equal(streamCarries("room-a"), true);
  h.session.socketEnded();
  await flush();
  assert.ok(h.open.has("room-a,room-b,room-x"));
});

test("S-D3: live ids are not collected when the stream REQ is refused before its EOSE or not sent", async () => {
  const h = harness();
  assert.equal(h.live.liveIds, null, "nothing before a connect");
  await h.session.connected(1);
  await h.session.requested;
  h.live.noteLive("x1");
  assert.deepEqual([...h.live.liveIds], ["x1"], "collected while pending");
  h.session.closed(h.host.sent.at(-1)[1], "unsupported: user-stream off");
  assert.equal(h.live.liveIds, null);
  h.live.noteLive("x2");
  assert.equal(h.live.liveIds, null, "stream off costs nothing");
  // Backed off: the REQ is not sent, nothing is collected.
  h.session.socketEnded();
  h.session.backoff.restore(JSON.stringify({ closes: [], until: L + 3600 }));
  const sent = h.host.sent.length;
  await h.session.connected(1);
  await h.session.requested;
  assert.equal(h.host.sent.length, sent, "backed off");
  h.live.noteLive("x3");
  assert.equal(h.live.liveIds, null);
});

test("membership: a rejoin of a room still in the EOSE catch-up gets its span only when the batch completes", async () => {
  let release;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  const h = harness({
    resume: { "room-a": L - 100 },
    pages: async () => {
      await gate;
      return [];
    },
  });
  await h.session.connected(1);
  await h.session.requested;
  const subId = h.host.sent.at(-1)[1];
  h.session.event(subId, part1("live", { "room-a": 1 }));
  h.session.eose(subId);
  for (let i = 0; i < 5; i += 1) await flush();
  h.host.now = (L + 10) * 1000;
  h.session.membershipNotice(roomEvent("l1", "room-a", L + 5, 44101));
  h.session.membershipNotice(roomEvent("j1", "room-a", L + 5, 44100));
  await flush();
  assert.equal(streamCarries("room-a"), true);
  assert.deepEqual(recorded, [], "pending: no span at the rejoin");
  release();
  await h.live.settled;
  await flush();
  assert.deepEqual(recorded, [[["room-a"], L + 10, L + 10]]);
});
