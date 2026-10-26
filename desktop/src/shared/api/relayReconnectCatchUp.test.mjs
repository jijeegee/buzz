import assert from "node:assert/strict";
import test from "node:test";

globalThis.window = {
  setTimeout: (fn) => {
    fn();
    return 0;
  },
  clearTimeout: () => {},
};

const { replayLiveSubscriptions, RECONNECT_REPLAY_CHANNEL_LOOKBACK_SECS } =
  await import("./relayReconnectReplay.ts");
const { buildChannelFilter } = await import("./relayChannelFilters.ts");
const { handleSubscriptionEose } = await import("./relayClosedRecovery.ts");

const ROOM = "channel-1";

function roomEvent(id, createdAt, room = ROOM, kind = 9) {
  return {
    id,
    pubkey: "pubkey",
    created_at: createdAt,
    kind,
    tags: [["h", room]],
    content: "",
    sig: "sig",
  };
}

function liveSub(delivered, extra = {}) {
  return {
    mode: "live",
    filter: buildChannelFilter(ROOM, 50),
    onEvent: (event) => delivered.push(event.id),
    lastSeenCreatedAt: 10_000,
    liveGeneration: 1,
    ...extra,
  };
}

function caughtUp(extra = {}) {
  return {
    status: "caughtUp",
    since: 9_000,
    events: [
      roomEvent("new", 10_500),
      roomEvent("other-room", 10_400, "channel-2"),
      roomEvent("old", 10_000 - RECONNECT_REPLAY_CHANNEL_LOOKBACK_SECS - 1),
      roomEvent("other-kind", 10_300, ROOM, 1),
    ],
    gapChannelIds: [],
    ...extra,
  };
}

async function replay(subscription, catchUp, options = {}) {
  const subscriptions = new Map([["live-1", subscription]]);
  const repairs = [];
  const catchUps = [];
  await replayLiveSubscriptions({
    subscriptions,
    sendRaw: async () => {},
    requestRepair: async (request) => {
      repairs.push(request);
      return [];
    },
    catchUp: async (ids) => {
      catchUps.push(ids);
      return catchUp();
    },
    generation: 2,
    ...options,
  });
  return { repairs, catchUps };
}

test("a catch-up stands in for the repair walk of a room live before", async () => {
  const delivered = [];
  const subscription = liveSub(delivered);
  const { repairs, catchUps } = await replay(subscription, () => caughtUp());
  assert.deepEqual(catchUps, [[ROOM]]);
  assert.deepEqual(repairs, []);
  // Only this room, the subscription's kinds, and the walk's own floor.
  assert.deepEqual(delivered, ["new"]);
  assert.equal(subscription.pendingReplaySince, undefined);
  assert.equal(subscription.reconnectReplay?.repairDone, true);
});

test("an older pinned floor the catch-up reaches is resolved by it", async () => {
  const delivered = [];
  const subscription = liveSub(delivered, { pendingReplaySince: 9_500 });
  const { repairs } = await replay(subscription, () => caughtUp());
  assert.deepEqual(repairs, []);
  assert.equal(subscription.pendingReplaySince, undefined);
});

for (const [name, subscriptionExtra, result] of [
  ["never live on an earlier connection", { liveGeneration: undefined }, null],
  ["live only on this connection", { liveGeneration: 2 }, null],
  ["a pinned floor below the catch-up", { pendingReplaySince: 8_000 }, null],
  ["a gap room", {}, { gapChannelIds: [ROOM] }],
  ["a fresh store", {}, { status: "fresh", since: null }],
  ["a stale watermark", {}, { status: "stale", since: null }],
  ["an unavailable socket", {}, { status: "unavailable", since: null }],
]) {
  test(`the repair walk runs for ${name}`, async () => {
    const subscription = liveSub([], subscriptionExtra);
    const { repairs } = await replay(subscription, () =>
      caughtUp(result ?? {}),
    );
    assert.equal(repairs.length, 1);
    assert.equal(repairs[0].channelId, ROOM);
  });
}

test("a failed or absent catch-up leaves every room to its walk", async () => {
  const failed = await replay(liveSub([]), () => {
    throw new Error("boom");
  });
  assert.equal(failed.repairs.length, 1);
  const absent = await replay(liveSub([]), () => null);
  assert.equal(absent.repairs.length, 1);
});

test("a catch-up runs even with no room to walk", async () => {
  const { catchUps } = await replay(
    liveSub([], { lastSeenCreatedAt: undefined }),
    () => caughtUp(),
  );
  assert.deepEqual(catchUps, [[]]);
});

test("a superseded connection neither dispatches nor walks", async () => {
  let active = true;
  const delivered = [];
  const { repairs } = await replay(
    liveSub(delivered),
    () => {
      active = false;
      return caughtUp();
    },
    { isActive: () => active },
  );
  assert.deepEqual(repairs, []);
  assert.deepEqual(delivered, []);
});

test("EOSE marks the live generation unless a CLOSED broke delivery", () => {
  const subscription = liveSub([], { liveGeneration: undefined });
  const subscriptions = new Map([["live-1", subscription]]);
  const eose = (generation) =>
    handleSubscriptionEose({
      subscriptions,
      subId: "live-1",
      closeSubscription: async () => {},
      generation,
    });
  eose(3);
  assert.equal(subscription.liveGeneration, 3);
  subscription.liveGeneration = undefined;
  subscription.liveGap = true;
  eose(4);
  assert.equal(subscription.liveGeneration, undefined);
});

test("a completed walk closes a live gap", async () => {
  const subscription = liveSub([], {
    liveGeneration: undefined,
    liveGap: true,
  });
  const { repairs } = await replay(subscription, () => caughtUp());
  assert.equal(repairs.length, 1);
  assert.equal(subscription.liveGap, undefined);
});

test("the connection is usable once REQs are restored, before the catch-up ends", async () => {
  const events = [];
  let finishCatchUp = () => {};
  const done = replayLiveSubscriptions({
    subscriptions: new Map([["live-1", liveSub([])]]),
    sendRaw: async (payload) => {
      events.push(payload[0]);
    },
    requestRepair: async () => [],
    catchUp: () => {
      events.push("catch-up");
      return new Promise((resolve) => {
        finishCatchUp = () => resolve(caughtUp());
      });
    },
    generation: 2,
    onRequestsRestored: () => events.push("restored"),
  });
  for (let i = 0; i < 10; i++) await Promise.resolve();
  assert.deepEqual(events, ["REQ", "restored", "catch-up"]);
  finishCatchUp();
  await done;
});
