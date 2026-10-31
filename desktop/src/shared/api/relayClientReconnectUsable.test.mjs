// A reconnect makes the socket usable once its live REQs are re-sent: sends,
// new REQs and history requests do not wait for the repair walks, which still
// complete behind them and grant no store certainty before they end
// (docs/message-sync.md#reconnect, #coverage). Drives the production
// connect() over a fake IPC; no native app or relay is used.
import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";

const NOW_SECS = 2_000;
const LAST_SEEN = 1_990;
const timers = new Map();
let nextTimerId = 1;
const writes = [];
let repairCalls = [];
globalThis.window = {
  setTimeout(fn, ms) {
    const id = nextTimerId++;
    timers.set(id, { fn, ms });
    return id;
  },
  clearTimeout: (id) => timers.delete(id),
  setInterval(fn, ms) {
    const id = nextTimerId++;
    timers.set(id, { fn, ms });
    return id;
  },
  clearInterval: (id) => timers.delete(id),
  __TAURI_INTERNALS__: {
    transformCallback: () => 1,
    async invoke(command, args) {
      if (command === "get_relay_ws_url") return "wss://relay.example";
      if (command === "plugin:websocket|connect") return 7;
      if (command === "plugin:websocket|send") {
        writes.push(JSON.parse(args.message.data));
        return;
      }
      if (command === "get_channel_reconnect_repair") {
        const call = Promise.withResolvers();
        repairCalls.push({ args, ...call });
        return call.promise;
      }
      if (command === "sign_event") {
        return JSON.stringify({
          id: "sent-1",
          pubkey: "me",
          created_at: NOW_SECS,
          kind: args.kind,
          tags: args.tags,
          content: args.content,
          sig: "sig",
        });
      }
      if (
        command === "plugin:websocket|disconnect" ||
        command === "plugin:websocket|close"
      ) {
        return;
      }
      assert.fail(`unexpected IPC: ${command}`);
    },
  },
};
Date.now = () => NOW_SECS * 1_000;

const { RelayClient } = await import("./relayClientSession.ts");
const { CHANNEL_EVENT_KINDS } = await import("../constants/kinds.ts");
const { RECONNECT_REPLAY_CHANNEL_LOOKBACK_SECS } = await import(
  "./relayReconnectReplay.ts"
);
const { setStoreCoverageDeps, waitStoreLive } = await import(
  "./relayStoreCoverage.ts"
);
const { resetRateLimitGate } = await import("./relayRateLimitGate.ts");

const extended = [];
let restoreDeps = () => {};
const clients = [];

beforeEach(() => {
  resetRateLimitGate();
  timers.clear();
  writes.length = 0;
  repairCalls = [];
  extended.length = 0;
  restoreDeps = setStoreCoverageDeps({
    nowSecs: () => NOW_SECS,
    nowMs: () => 0,
    // Never times out by itself: the wait below ends only with the repair.
    sleep: () => {
      let cancel = () => {};
      const done = new Promise((resolve) => {
        cancel = resolve;
      });
      return { done, cancel };
    },
    scope: async () => "scope-1",
    extend: async (roomIds, from, to) => {
      extended.push([roomIds, from, to]);
    },
    resumePoints: async () => ({}),
  });
});
afterEach(() => {
  for (const client of clients.splice(0)) client.disconnect();
  restoreDeps();
  resetRateLimitGate();
});

async function flush() {
  for (let i = 0; i < 50; i++) await Promise.resolve();
}

const chunkFilter = { kinds: CHANNEL_EVENT_KINDS, "#h": ["room-a"], limit: 0 };

/** A client whose room-bundle chunk was live on a socket that dropped. */
async function reconnectWithPendingRepair() {
  const client = new RelayClient();
  clients.push(client);
  client.hasConnectedOnce = true;
  client.subscriptions.set("live-chunk", {
    mode: "live",
    filter: chunkFilter,
    onEvent: () => {},
    lastSeenCreatedAt: LAST_SEEN,
    liveSince: { "room-a": LAST_SEEN },
  });
  let connected = false;
  const connecting = client.ensureConnected().then(() => {
    connected = true;
  });
  await flush();
  assert.ok(client.authRequest, "connect waits for AUTH");
  client.authRequest.resolve();
  await flush();
  return { client, connecting, isConnected: () => connected };
}

test("sends, new REQs and history go out while the repair walk is unfinished", async () => {
  const { client, connecting, isConnected } =
    await reconnectWithPendingRepair();
  assert.deepEqual(
    writes.map((frame) => frame[0]),
    ["REQ"],
    "the live chunk is re-sent first",
  );
  assert.equal(repairCalls.length, 1, "the repair walk has started");
  assert.ok(isConnected(), "connect resolves before the walk ends");
  await connecting;

  const history = client.fetchEvents({
    kinds: [9],
    "#h": ["room-b"],
    limit: 20,
  });
  void history.catch(() => {});
  void client
    .subscribeLive({ kinds: [9], "#h": ["room-b"], limit: 0 }, () => {})
    .catch(() => {});
  const send = client.sendMessage("room-b", "hello");
  void send.catch(() => {});
  await flush();

  const reqs = writes.filter((frame) => frame[0] === "REQ");
  assert.equal(reqs.length, 3, "history and live REQs went out");
  assert.deepEqual(reqs[1][2], { kinds: [9], "#h": ["room-b"], limit: 20 });
  assert.deepEqual(reqs[2][2], { kinds: [9], "#h": ["room-b"], limit: 0 });
  assert.deepEqual(
    writes.filter((frame) => frame[0] === "EVENT").map((f) => f[1].id),
    ["sent-1"],
    "the send went out",
  );
  assert.equal(repairCalls.length, 1, "still the same unfinished walk");

  repairCalls[0].resolve([]);
  await flush();
});

test("the repair still completes, and the room is not certain before it ends", async () => {
  const { client } = await reconnectWithPendingRepair();
  const since = LAST_SEEN - RECONNECT_REPLAY_CHANNEL_LOOKBACK_SECS;
  assert.equal(repairCalls[0].args.since, since);
  await client.handleWsMessage(
    { type: "Text", data: JSON.stringify(["EOSE", "live-chunk"]) },
    client.connectionGeneration,
  );
  await flush();

  let settled = false;
  const waiting = waitStoreLive(["room-a"]).then(() => {
    settled = true;
  });
  await flush();
  assert.equal(settled, false, "the preload wait holds while the walk runs");
  assert.deepEqual(
    extended,
    [[["room-a"], NOW_SECS, NOW_SECS]],
    "only the EOSE point is covered, not the missed window",
  );
  const subscription = client.subscriptions.get("live-chunk");
  assert.equal(subscription.pendingReplaySince, since);

  repairCalls[0].resolve([]);
  await flush();
  await waiting;
  assert.ok(settled);
  assert.deepEqual(extended.at(-1), [["room-a"], since, NOW_SECS]);
  assert.equal(subscription.pendingReplaySince, undefined);
});
