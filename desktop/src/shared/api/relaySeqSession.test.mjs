// The production relay session against a kuzz-seq relay: frame numbers,
// bounds events, KUZZ_RESYNC and the opt-in gate. The IPC transport is fake.
import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";

const CH = "270f6caf-0feb-4055-93f3-cdbeb567ff28";
const writes = [];
const repairs = [];
let repairPage = () => [];
globalThis.window = {
  setTimeout: () => 1,
  clearTimeout() {},
  __TAURI_INTERNALS__: {
    async invoke(command, args) {
      if (command === "plugin:websocket|send") {
        writes.push(JSON.parse(args.message.data));
        return;
      }
      if (command === "plugin:websocket|disconnect") return;
      if (command === "get_channel_reconnect_repair") {
        repairs.push(args);
        return repairPage(args);
      }
      assert.fail(`unexpected IPC: ${command}`);
    },
  },
};
const { RelayClient } = await import("./relayClientSession.ts");
const { resetRateLimitGate } = await import("./relayRateLimitGate.ts");
const { buildChannelFilter } = await import("./relayChannelFilters.ts");

const clients = [];
beforeEach(() => {
  resetRateLimitGate();
  writes.length = 0;
  repairs.length = 0;
  repairPage = () => [];
});
afterEach(() => {
  for (const client of clients.splice(0)) client.disconnect();
});

async function flush() {
  for (let i = 0; i < 30; i++) await Promise.resolve();
}
function deliver(client, frame) {
  return client.handleWsMessage(
    { type: "Text", data: JSON.stringify(frame) },
    client.connectionGeneration,
  );
}
function event(id, extra = {}) {
  return {
    id,
    pubkey: "p",
    created_at: 100,
    kind: 9,
    tags: [["h", CH]],
    content: "",
    sig: "s",
    ...extra,
  };
}

async function session(optedIn) {
  const client = new RelayClient();
  client.wsId = 7;
  const url = "wss://relay.test";
  client.seq.connected(
    client.connectionGeneration,
    optedIn ? `${url}?kuzz_seq=1` : url,
    url,
  );
  clients.push(client);
  const received = [];
  const pending = client.subscribeLive(
    { ...buildChannelFilter(CH, 1000), since: 1 },
    (e) => received.push(e),
  );
  await flush();
  const req = writes.find((frame) => frame[0] === "REQ");
  await deliver(client, ["EOSE", req[1]]);
  await pending;
  return { client, received, subId: req[1] };
}

test("socket URL opts in only when the relay advertises kuzz-seq", async () => {
  const client = new RelayClient();
  clients.push(client);
  client.seq.probe = async () => true;
  assert.equal(
    await client.seq.socketUrl("wss://relay.test"),
    "wss://relay.test?kuzz_seq=1",
  );
  client.seq.probe = async () => false;
  assert.equal(await client.seq.socketUrl("wss://relay.test"), "wss://relay.test");
});

test("opted-in EVENT frames carry their number and feed the cursor", async () => {
  const { client, received, subId } = await session(true);
  await deliver(client, ["EVENT", subId, event("a"), { channel_sequence: 41 }]);
  client.flushEventBuffer();
  assert.deepEqual(
    received.map((e) => [e.id, e.seq]),
    [["a", 41]],
  );
  assert.equal(client.seq.cursors.replayPoint(CH, 0), 40);
});

test("an after_seq bounds event never reaches live handlers", async () => {
  const { client, received, subId } = await session(true);
  await deliver(client, [
    "EVENT",
    subId,
    event("b", {
      kind: 39006,
      tags: [["d", `${CH}:after_seq:0`]],
      content: JSON.stringify({ order: "seq", last_seq: 3, has_more: false }),
    }),
  ]);
  client.flushEventBuffer();
  assert.deepEqual(received, []);
});

test("KUZZ_RESYNC repairs known channels by number without a new REQ", async () => {
  const { client, received, subId } = await session(true);
  await deliver(client, ["EVENT", subId, event("a"), { channel_sequence: 41 }]);
  client.flushEventBuffer();
  writes.length = 0;
  repairPage = () => [
    event("missed"),
    event("bounds", {
      kind: 39006,
      tags: [["d", `${CH}:after_seq:40`]],
      content: JSON.stringify({
        order: "seq",
        after_seq: 40,
        last_seq: 42,
        has_more: false,
        sequences: [["missed", 42]],
      }),
    }),
  ];
  await deliver(client, ["KUZZ_RESYNC", { reason: "redis_reconnect" }]);
  await flush();
  assert.deepEqual(writes, [], "live REQs stay open");
  assert.deepEqual(repairs, [
    {
      channelId: CH,
      since: null,
      afterSeq: 40,
      limit: 500,
      until: null,
      beforeId: null,
    },
  ]);
  assert.deepEqual(
    received.map((e) => [e.id, e.seq]),
    [
      ["a", 41],
      ["missed", 42],
    ],
  );
});

test("a session that did not opt in ignores numbers and KUZZ_RESYNC", async () => {
  const { client, received, subId } = await session(false);
  await deliver(client, ["EVENT", subId, event("a"), { channel_sequence: 41 }]);
  client.flushEventBuffer();
  await deliver(client, ["KUZZ_RESYNC", { reason: "lag" }]);
  await flush();
  assert.equal(received[0].seq, undefined);
  assert.equal(client.seq.cursors.get(CH), undefined);
  assert.deepEqual(repairs, []);
});

test("an accepted publish takes the number from its OK frame", async () => {
  const { client } = await session(true);
  const published = event("mine");
  client.pendingEvents.set("mine", {
    event: published,
    resolve() {},
    reject() {},
    timeout: 1,
  });
  await deliver(client, ["OK", "mine", true, "", { channel_sequence: 77 }]);
  assert.equal(published.seq, 77);
  assert.notEqual(client.seq.cursors.get(CH), undefined);
});
