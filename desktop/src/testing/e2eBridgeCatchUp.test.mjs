import assert from "node:assert/strict";
import test from "node:test";

import { mockChannelsCatchUp } from "./e2eBridgeCatchUp.ts";
import { createMockStoreTap } from "./e2eBridgeSubscriptions.ts";
import { createMockMessageStore, MOCK_SYNC_KINDS } from "./e2eMessageStore.ts";

const ROOM = "270f6caf-0feb-4055-93f3-cdbeb567ff28";
const id = (n) => n.toString(16).padStart(2, "0").repeat(32);

function scope() {
  return createMockMessageStore(null).scope({
    pubkey: id(1),
    relayUrl: "ws://localhost:3000",
  });
}

function roomEvent(n, createdAt) {
  return {
    id: id(n),
    pubkey: id(2),
    created_at: createdAt,
    kind: 9,
    tags: [["h", ROOM]],
    content: "",
    sig: "",
  };
}

test("mock tap moves the watermark only once caught up, like the native tap", () => {
  const store = scope();
  let clock = 500;
  const tap = createMockStoreTap(store, 3, MOCK_SYNC_KINDS, () => clock);
  const req = JSON.stringify([
    "REQ",
    "live-1",
    { kinds: MOCK_SYNC_KINDS, "#h": [ROOM], since: 100, limit: 1000 },
  ]);
  tap.outgoing(req);
  tap.incoming(JSON.stringify(["EOSE", "live-1"]));
  assert.equal(store.watermark(), null);

  assert.equal(tap.markCaughtUp(), true);
  clock = 600;
  tap.incoming(JSON.stringify(["EVENT", "live-1", roomEvent(1, 550)]));
  assert.equal(store.watermark(), 550);
  // A future timestamp vouches for no more than the frame time.
  tap.incoming(JSON.stringify(["EVENT", "live-1", roomEvent(2, 9_999)]));
  assert.equal(store.watermark(), 600);

  clock = 700;
  tap.outgoing(JSON.stringify(["REQ", "probe", { kinds: [9], limit: 1 }]));
  clock = 710;
  tap.incoming(JSON.stringify(["EOSE", "probe"]));
  assert.equal(store.watermark(), 700);

  tap.finish();
  assert.equal(tap.markCaughtUp(), false);
});

test("mock catch-up: fresh, then caught up from the watermark with claims", async () => {
  const store = scope();
  const tap = createMockStoreTap(store, 3, MOCK_SYNC_KINDS, () => 10_000);
  const filters = [];
  const query = async (filter) => {
    filters.push(filter);
    return [roomEvent(1, 9_500)];
  };
  const fresh = await mockChannelsCatchUp({
    store,
    tap,
    channelIds: [ROOM],
    dispatchChannelIds: [],
    syncKinds: MOCK_SYNC_KINDS,
    query,
    now: () => 10_000,
  });
  assert.equal(fresh.status, "fresh");
  assert.equal(filters.length, 0);
  assert.equal(store.watermark(), 10_000);

  const result = await mockChannelsCatchUp({
    store,
    tap,
    channelIds: [],
    dispatchChannelIds: [ROOM],
    syncKinds: MOCK_SYNC_KINDS,
    query,
    now: () => 10_100,
  });
  assert.equal(result.status, "caughtUp");
  assert.equal(result.since, 10_000 - 1865);
  assert.deepEqual(
    result.events.map((event) => event.id),
    [id(1)],
  );
  assert.deepEqual(filters[0]["#h"], [ROOM]);
  assert.equal(filters[0].since, 10_000 - 1865);
  assert.equal(store.watermark(), 10_100);
  assert.equal(store.coverage(ROOM).provenThrough, 10_100);
});

test("mock catch-up overflow is a gap and an unknown socket is unavailable", async () => {
  const store = scope();
  store.advanceWatermark(10_000);
  const tap = createMockStoreTap(store, 3, MOCK_SYNC_KINDS, () => 10_000);
  const full = Array.from({ length: 1000 }, (_, n) =>
    roomEvent(n % 250, 9_500),
  );
  const result = await mockChannelsCatchUp({
    store,
    tap,
    channelIds: [ROOM],
    dispatchChannelIds: [],
    syncKinds: MOCK_SYNC_KINDS,
    query: async () => full,
    now: () => 10_100,
  });
  assert.deepEqual(result.gapChannelIds, [ROOM]);
  const unavailable = await mockChannelsCatchUp({
    store,
    tap: undefined,
    channelIds: [ROOM],
    dispatchChannelIds: [],
    syncKinds: MOCK_SYNC_KINDS,
    query: async () => [],
  });
  assert.equal(unavailable.status, "unavailable");
});
