import assert from "node:assert/strict";
import test from "node:test";
import {
  createMockSubscription,
  hasMockSubscription,
} from "./e2eBridgeSubscriptions.ts";

const ready = (filters, channel = "channel", kind = 9, exact = true) =>
  hasMockSubscription([createMockSubscription(filters)], channel, kind, exact);

test("channel readiness rejects global-only, wrong-channel and wrong-kind REQs", () => {
  for (const filter of [
    { kinds: [9] },
    { "#h": [], kinds: [9] },
    { "#h": ["other"], kinds: [9] },
    { "#h": ["channel"], kinds: [30078] },
  ])
    assert.equal(ready([filter]), false);
  assert.equal(hasMockSubscription([], "channel", 9, true), false);
  for (const kinds of [[9], [7, 9], undefined]) {
    assert.equal(ready([{ "#h": ["channel"], kinds }]), true);
  }
  const emptyKinds = [
    createMockSubscription([{ "#h": ["channel"], kinds: [] }]),
  ];
  assert.equal(hasMockSubscription(emptyKinds, "channel", 9, true), false);
  assert.equal(
    hasMockSubscription(emptyKinds, "channel", undefined, true),
    false,
  );
  assert.equal(
    ready([
      { "#h": ["channel"], kinds: [] },
      { "#h": ["channel"], kinds: [9] },
    ]),
    true,
  );
});

test("REQ storage preserves channel/kind correlation across OR filters", () => {
  for (const unrelated of [{ kinds: [9] }, { "#h": ["other"], kinds: [9] }]) {
    const filters = [{ "#h": ["channel"], kinds: [30078] }, unrelated];
    const stored = createMockSubscription(filters);
    assert.deepEqual(stored.filters, filters);
    assert.equal(hasMockSubscription([stored], "channel", 9, true), false);
    assert.equal(hasMockSubscription([stored], "channel", 30078, true), true);
    assert.equal(ready([...filters, { "#h": ["channel"], kinds: [9] }]), true);
  }
});

test("legacy readiness and explicit global queries retain their semantics", () => {
  const global = [createMockSubscription([{ kinds: [30078] }])];
  assert.equal(hasMockSubscription(global, "channel"), true);
  assert.equal(hasMockSubscription(global, "channel", 9), false);
  assert.equal(hasMockSubscription(global, "channel", 30078), true);
  assert.equal(hasMockSubscription(global, "*", 30078), true);
  assert.equal(hasMockSubscription(global, "*", 9), false);
  assert.equal(hasMockSubscription(global, "channel", 30078, true), false);
  assert.equal(
    ready(
      [{ "#h": ["channel"], kinds: [30078] }, { kinds: [9] }],
      "channel",
      9,
      false,
    ),
    true,
  );
});

test("mock store tap stores room events and proves live rooms until close", async () => {
  const { createMockStoreTap } = await import("./e2eBridgeSubscriptions.ts");
  const { createMockMessageStore, MOCK_SYNC_KINDS } = await import(
    "./e2eMessageStore.ts"
  );
  const room = "270f6caf-0feb-4055-93f3-cdbeb567ff28";
  const id = (n) => n.toString(16).padStart(2, "0").repeat(32);
  const store = createMockMessageStore(null).scope({
    pubkey: id(1),
    relayUrl: "ws://localhost:3000",
  });
  let clock = 500;
  const tap = createMockStoreTap(store, 3, MOCK_SYNC_KINDS, () => clock);
  const event = (n, tags) =>
    JSON.stringify([
      "EVENT",
      "live-1",
      {
        id: id(n),
        pubkey: id(2),
        created_at: 400 + n,
        kind: 9,
        tags,
        content: "",
        sig: "",
      },
    ]);
  tap.outgoing(
    JSON.stringify([
      "REQ",
      "live-1",
      {
        kinds: [...MOCK_SYNC_KINDS, 39005],
        "#h": [room],
        since: 100,
        limit: 1000,
      },
    ]),
  );
  tap.incoming(event(1, [["h", room]]));
  tap.incoming(event(2, [["p", id(3)]]));
  tap.incoming(JSON.stringify(["EOSE", "live-1"]));
  // Open segment: nothing is proven until it closes.
  assert.equal(store.coverage(room).provenThrough, null);
  clock = 600;
  tap.incoming(event(3, [["h", room]]));
  tap.finish();
  const coverage = store.coverage(room);
  assert.equal(coverage.provenThrough, 600);
  assert.deepEqual(
    coverage.ranges
      .filter((range) => range.class === "all")
      .map((range) => [range.loAt, range.hiAt]),
    [[100, 600]],
  );
  assert.deepEqual(
    store.readWindow(room, null, 10).rows.map((row) => row.id),
    [id(3), id(1)],
  );
});
