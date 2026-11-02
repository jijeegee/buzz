import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, test } from "node:test";

import { RoomLiveBundle } from "./relayRoomBundle.ts";
import {
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
  baseline = null,
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
  };
  const store = {
    epoch: async (value) => {
      calls.epoch.push(value);
      return true;
    },
    seqs: async (roomIds) => ({
      epoch,
      rows: Object.fromEntries(
        roomIds.filter((room) => rows[room]).map((room) => [room, rows[room]]),
      ),
    }),
    clearRows: async (roomIds) => {
      calls.clear.push(roomIds);
    },
    scope: async () => "scope-1",
    // A number is a span ending there; a list holds `[lo, hi]` spans, of
    // which only those beginning before `before` count (as in the store).
    resumePoints: async (roomIds, before) => {
      calls.before.push(before);
      return Object.fromEntries(
        roomIds.flatMap((room) => {
          const spans = resume[room];
          if (spans === undefined) return [];
          if (!Array.isArray(spans)) return [[room, spans]];
          const his = spans.filter(([lo]) => lo < before).map(([, hi]) => hi);
          return his.length === 0 ? [] : [[room, Math.max(...his)]];
        }),
      );
    },
    baseline: async () => baseline,
    extend: async (roomIds, from, to, scope, seq) => {
      calls.extend.push({ roomIds, from, to, scope, seq });
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
    log: (line) => host.logs.push(line),
    random: () => 0,
  });
  session.liveHooks = live;
  const start = async (mode, snapshotRooms, connectAt = L) => {
    host.now = connectAt * 1000;
    await session.connected(1);
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
  const h = harness();
  h.bundle.setMemberRooms(["room-a", "room-b", "room-c"]);
  void h.bundle.listenRoom("room-x", () => {});
  await flush();
  assert.deepEqual([...h.open.keys()].sort(), [
    "room-a,room-b,room-c",
    "room-x",
  ]);
  const subId = await h.start("live", { "room-a": 1, "room-b": 2 });
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
    baseline: { "room-a": { segments: [[1_990_000, 1_999_000]], ids: [] } },
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
    resume: { "room-a": 1_999_000 },
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
    [[["room-a"], 1_999_000 - 1865, closedAt, undefined]],
  );
  assert.deepEqual(
    shown,
    ["r1"],
    "rows before the live start minus 5 s stay out",
  );
  assert.equal(h.session.report.counters.fallback_closed, 1);
});

test("an overflow in live waits for the socket: no chunk opens until it ends", async () => {
  const h = harness();
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
  const h = harness();
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
  assert.deepEqual(opened, ["ar1"]);
  assert.deepEqual(all, []);
});

test("the stream's span grows at most once a minute and stops at its CLOSED", async () => {
  const h = harness();
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
    baseline: { "room-a": { segments: [[L - 6000, L - 1500]], ids: [] } },
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
    baseline: {
      "room-a": {
        segments: [
          [L - 30_000, L - 7200],
          [L - 2, L - 2],
        ],
        ids: [],
      },
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
  const h = harness();
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
  const first = h.session.connected(1);
  h.session.scopeChanged();
  await h.session.connected(1);
  assert.equal(h.host.sent.length, 1, "account B's REQ is sent");
  release();
  await first;
  assert.equal(h.host.sent.length, 1, "account A's late restore sends nothing");
  assert.equal(h.session.report.counters.reconnects ?? 0, 0);
  h.session.networkChange();
  assert.deepEqual(JSON.parse(h.calls.saved.stream_backoff).until, 0);
});
