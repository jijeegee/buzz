// A bundle's multi-filter live REQ on the real session. Only the IPC
// transport and clock are fake.
import assert from "node:assert/strict";
import { after, afterEach, beforeEach, test } from "node:test";

const originalNow = Date.now;
const originalWindow = globalThis.window;
let now = 0;
let nextTimerId = 1;
const timers = new Map();
const writes = [];
const clients = [];
globalThis.window = {
  setTimeout(fn, ms) {
    const id = nextTimerId++;
    timers.set(id, { fn, at: now + ms });
    return id;
  },
  clearTimeout(id) {
    timers.delete(id);
  },
  __TAURI_INTERNALS__: {
    async invoke(command, args) {
      if (command === "plugin:websocket|send") {
        writes.push({ socket: args.id, frame: JSON.parse(args.message.data) });
        return;
      }
      if (command === "plugin:websocket|disconnect") return;
      assert.fail(`unexpected IPC: ${command}`);
    },
  },
};
Date.now = () => now;
const { RelayClient } = await import("./relayClientSession.ts");
const { assertBundleLiveFilters, assertRoomLiveFilters } = await import(
  "./relayLiveFilters.ts"
);
const { resetRateLimitGate } = await import("./relayRateLimitGate.ts");

beforeEach(() => {
  resetRateLimitGate();
  now = 0;
  timers.clear();
  writes.length = 0;
});
afterEach(() => {
  for (const client of clients.splice(0)) client.disconnect();
  resetRateLimitGate();
});
after(() => {
  Date.now = originalNow;
  globalThis.window = originalWindow;
});

async function flush() {
  for (let i = 0; i < 20; i++) await Promise.resolve();
}
async function tickTo(target) {
  for (;;) {
    const next = [...timers]
      .filter(([, timer]) => timer.at <= target)
      .sort((a, b) => a[1].at - b[1].at || a[0] - b[0])[0];
    if (!next) break;
    now = next[1].at;
    timers.delete(next[0]);
    next[1].fn();
    await flush();
  }
  now = target;
  await flush();
}
function deliver(client, frame) {
  return client.handleWsMessage(
    { type: "Text", data: JSON.stringify(frame) },
    client.connectionGeneration,
  );
}
const reqs = () => writes.filter((write) => write.frame[0] === "REQ");

const STATUS = { kinds: [30315], "#d": ["general"], limit: 0 };
const presence = (...authors) => ({ kinds: [20001], authors, limit: 0 });
const A = "a".repeat(64);
const B = "b".repeat(64);
const C = "c".repeat(64);

async function open(filters = [STATUS]) {
  const client = new RelayClient();
  client.wsId = 7;
  clients.push(client);
  const events = [];
  const pending = client.subscribeLiveFilters(filters, (e) => events.push(e));
  await flush();
  await tickTo(now + 10);
  const subId = reqs()[0].frame[1];
  await deliver(client, ["EOSE", subId]);
  const handle = await pending;
  return { client, handle, subId, events };
}

test("bundle filters are live only, at most ten", () => {
  assert.throws(() => assertBundleLiveFilters([]));
  assert.throws(() => assertBundleLiveFilters([{ kinds: [1], limit: 5 }]));
  assert.throws(() =>
    assertBundleLiveFilters([{ kinds: [9], "#h": ["room"], limit: 0 }]),
  );
  assert.throws(() =>
    assertBundleLiveFilters([{ kinds: [1], since: 5, limit: 0 }]),
  );
  assert.throws(() =>
    assertBundleLiveFilters(
      Array.from({ length: 11 }, (_, i) => ({ kinds: [i], limit: 0 })),
    ),
  );
  assertBundleLiveFilters([STATUS, presence(A)]);
});

test("the opening REQ carries every filter and events reach the listener", async () => {
  const { client, subId, events } = await open([STATUS, presence(A)]);
  assert.deepEqual(reqs()[0].frame, ["REQ", subId, STATUS, presence(A)]);
  await deliver(client, [
    "EVENT",
    subId,
    { id: "e1", kind: 30315, pubkey: B, tags: [], content: "", created_at: 1 },
  ]);
  await tickTo(now + 100);
  assert.deepEqual(
    events.map((e) => e.id),
    ["e1"],
  );
});

test("setFilters after EOSE goes out at once under the same subId", async () => {
  const { client, handle, subId } = await open([STATUS, presence(A)]);
  let resolved = false;
  const done = handle.setFilters([STATUS, presence(A, B)]).then(() => {
    resolved = true;
  });
  await flush();
  assert.equal(reqs().length, 2);
  assert.deepEqual(reqs()[1].frame, ["REQ", subId, STATUS, presence(A, B)]);
  assert.equal(resolved, false);
  await deliver(client, ["EOSE", subId]);
  await done;
  assert.equal(resolved, true);
  // The same set again sends nothing.
  await handle.setFilters([STATUS, presence(A, B)]);
  assert.equal(reqs().length, 2);
});

test("two setFilters before an EOSE send one more REQ, with the newest set", async () => {
  const { client, handle, subId } = await open([STATUS, presence(A)]);
  const first = handle.setFilters([STATUS, presence(A, B)]);
  await flush();
  assert.equal(reqs().length, 2);
  // In flight: both of these wait for the EOSE above.
  const second = handle.setFilters([STATUS, presence(A, B, C)]);
  const third = handle.setFilters([STATUS, presence(A, C)]);
  await flush();
  assert.equal(reqs().length, 2, "nothing goes out while a REQ is in flight");
  await deliver(client, ["EOSE", subId]);
  await first;
  await flush();
  assert.equal(reqs().length, 3);
  assert.deepEqual(reqs()[2].frame, ["REQ", subId, STATUS, presence(A, C)]);
  await deliver(client, ["EOSE", subId]);
  await Promise.all([second, third]);
  assert.equal(reqs().length, 3);
});

test("the opening REQ counts as in flight", async () => {
  const client = new RelayClient();
  client.wsId = 7;
  clients.push(client);
  const pending = client.subscribeLiveFilters([STATUS, presence(A)], () => {});
  await flush();
  await tickTo(now + 10);
  assert.equal(reqs().length, 1);
  const subId = reqs()[0].frame[1];
  // The readiness fallback resolves the handle before the relay answered.
  await tickTo(now + 300);
  const handle = await pending;
  const update = handle.setFilters([STATUS, presence(A, B)]);
  await flush();
  assert.equal(reqs().length, 1);
  await deliver(client, ["EOSE", subId]);
  await flush();
  assert.equal(reqs().length, 2);
  assert.deepEqual(reqs()[1].frame[3], presence(A, B));
  await deliver(client, ["EOSE", subId]);
  await update;
});

test("an update while the socket is down rides the replay, unchanged", async () => {
  const { client, handle, subId } = await open([STATUS, presence(A)]);
  client.resetConnection(new Error("socket lost"));
  window.clearTimeout(client.reconnectTimeout);
  client.reconnectTimeout = null;
  const update = handle.setFilters([STATUS, presence(A, B)]);
  await flush();
  assert.equal(reqs().length, 1, "nothing is sent while offline");
  client.wsId = 8;
  // A +290 s gap does not add `since` to live-only filters.
  now += 290_000;
  await client.replayLiveSubscriptions();
  client.reconnectWaiters.settle();
  await flush();
  const replay = reqs().filter((w) => w.socket === 8);
  assert.deepEqual(
    replay.map((w) => w.frame),
    [["REQ", subId, STATUS, presence(A, B)]],
  );
  await deliver(client, ["EOSE", subId]);
  await update;
  assert.equal(reqs().length, 2);
});

test("a CLOSED retry re-sends the current set", async () => {
  const { client, handle, subId } = await open([STATUS, presence(A)]);
  await deliver(client, ["CLOSED", subId, "error: temporary failure"]);
  const update = handle.setFilters([STATUS, presence(A, B)]);
  await flush();
  assert.equal(reqs().length, 1, "the retry, not setFilters, sends next");
  await tickTo(now + 1_500);
  assert.equal(reqs().length, 2);
  assert.deepEqual(reqs()[1].frame, ["REQ", subId, STATUS, presence(A, B)]);
  await deliver(client, ["EOSE", subId]);
  await update;
});

test("close resolves a pending update and sends CLOSE", async () => {
  const { client, handle, subId } = await open([STATUS, presence(A)]);
  const first = handle.setFilters([STATUS, presence(A, B)]);
  const second = handle.setFilters([STATUS, presence(A, B, C)]);
  await handle.close();
  await Promise.all([first, second]);
  assert.ok(
    writes.some((w) => w.frame[0] === "CLOSE" && w.frame[1] === subId),
  );
  assert.equal(client.subscriptions.has(subId), false);
});

const ROOM = "11111111-0000-4000-8000-000000000001";
const TYPING = { kinds: [20002], "#h": [ROOM], limit: 0 };
const GOALS = { kinds: [40110], "#h": [ROOM], limit: 0 };

async function openRoom(filters = [TYPING, GOALS], onGone) {
  const client = new RelayClient();
  client.wsId = 7;
  clients.push(client);
  const events = [];
  const pending = client.subscribeRoomLiveFilters(
    filters,
    (e) => events.push(e),
    undefined,
    undefined,
    onGone,
  );
  await flush();
  await tickTo(now + 10);
  const subId = reqs().at(-1).frame[1];
  await deliver(client, ["EOSE", subId]);
  const dispose = await pending;
  return { client, dispose, subId, events };
}

test("room filters are live only and name the same single room", () => {
  assert.throws(() => assertRoomLiveFilters([]));
  assert.throws(() => assertRoomLiveFilters([{ kinds: [20002], limit: 0 }]));
  assert.throws(() =>
    assertRoomLiveFilters([TYPING, { kinds: [40110], limit: 0 }]),
  );
  assert.throws(() =>
    assertRoomLiveFilters([TYPING, { ...GOALS, "#h": ["other"] }]),
  );
  assert.throws(() =>
    assertRoomLiveFilters([{ ...TYPING, "#h": [ROOM, "other"] }]),
  );
  assert.throws(() => assertRoomLiveFilters([{ ...TYPING, limit: 10 }]));
  assert.throws(() => assertRoomLiveFilters([{ ...TYPING, since: 5 }]));
  assert.throws(() => assertRoomLiveFilters([{ ...TYPING, until: 5 }]));
  assert.throws(() =>
    assertRoomLiveFilters(Array.from({ length: 11 }, () => TYPING)),
  );
  assertRoomLiveFilters([TYPING, GOALS]);
});

test("a room REQ carries both filters and replays them byte-identical", async () => {
  const { client, subId, events } = await openRoom();
  assert.deepEqual(reqs()[0].frame, ["REQ", subId, TYPING, GOALS]);
  const typing = {
    id: "1".repeat(64),
    pubkey: A,
    kind: 20002,
    content: "",
    created_at: 1,
    tags: [["h", ROOM]],
    sig: "",
  };
  await deliver(client, ["EVENT", subId, typing]);
  await tickTo(now + 100);
  assert.deepEqual(
    events.map((e) => e.id),
    [typing.id],
  );
  client.resetConnection(new Error("socket lost"));
  window.clearTimeout(client.reconnectTimeout);
  client.reconnectTimeout = null;
  client.wsId = 8;
  // A +290 s gap adds no `since`, even after a live typing event.
  now += 290_000;
  await client.replayLiveSubscriptions();
  client.reconnectWaiters.settle();
  await flush();
  assert.deepEqual(
    reqs()
      .filter((w) => w.socket === 8)
      .map((w) => w.frame),
    [["REQ", subId, TYPING, GOALS]],
  );
});

test("a CLOSED retry re-sends both room filters", async () => {
  const { client, subId } = await openRoom();
  await deliver(client, ["CLOSED", subId, "error: temporary failure"]);
  await tickTo(now + 1_500);
  assert.equal(reqs().length, 2);
  assert.deepEqual(reqs()[1].frame, ["REQ", subId, TYPING, GOALS]);
});

test("a terminal CLOSED drops the room REQ and calls onGone", async () => {
  let gone = 0;
  const { client, subId } = await openRoom(undefined, () => {
    gone += 1;
  });
  await deliver(client, ["CLOSED", subId, "restricted: not a member"]);
  await tickTo(now + 60_000);
  assert.equal(gone, 1);
  assert.equal(client.subscriptions.has(subId), false);
  assert.equal(reqs().length, 1, "no retry after a terminal CLOSED");
});

test("the open room's REQ drains before other cold REQs", async () => {
  const client = new RelayClient();
  client.wsId = 7;
  clients.push(client);
  client.setVisibleChannelId(ROOM);
  const quiet = (promise) => promise.catch(() => {});
  quiet(client.subscribeLive({ kinds: [1], limit: 0 }, () => {}));
  await flush();
  await tickTo(now + 10);
  assert.equal(reqs().length, 1, "the first REQ goes out at once");
  // Another cold `limit: 0` REQ queued first: only the visible room's
  // priority puts the room REQ ahead of it.
  quiet(client.subscribeLive({ kinds: [7], limit: 0 }, () => {}));
  quiet(client.subscribeRoomLiveFilters([TYPING, GOALS], () => {}));
  await flush();
  await tickTo(now + 300);
  assert.deepEqual(reqs()[1].frame.slice(2), [TYPING, GOALS]);
});
