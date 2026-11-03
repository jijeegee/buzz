import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// `error: busy` on history REQs (docs/message-sync.md#relay-answers,
// test-fixtures/user-stream.json `busyRetry`).
const fixture = JSON.parse(
  readFileSync(
    new URL("../../../../test-fixtures/user-stream.json", import.meta.url),
    "utf8",
  ),
);

let now = 0;
const timers = new Map();
let nextTimer = 1;
globalThis.window = {
  setTimeout: (fn, ms) => {
    const id = nextTimer++;
    timers.set(id, { fn, at: now + ms });
    return id;
  },
  clearTimeout: (id) => timers.delete(id),
};

function advance(ms) {
  const until = now + ms;
  for (;;) {
    const due = [...timers.entries()]
      .filter(([, timer]) => timer.at <= until)
      .sort(([, a], [, b]) => a.at - b.at)[0];
    if (!due) break;
    timers.delete(due[0]);
    now = due[1].at;
    due[1].fn();
  }
  now = until;
}

const flush = () => new Promise((resolve) => setImmediate(resolve));

const { handleRelayClosed, handleSubscriptionEose, flushEvents } = await import(
  "./relayClosedRecovery.ts"
);
const { requestFirstEventGated, requestHistoryGated } = await import(
  "./relayGateBoundary.ts"
);

function harness() {
  const subscriptions = new Map();
  const sent = [];
  const sendRaw = async (frame) => {
    sent.push({ frame, at: now });
  };
  const closed = (subId) =>
    handleRelayClosed({
      subscriptions,
      subId,
      message: "error: busy",
      sendReq: (id, filter) => sendRaw(["REQ", id, filter]),
      closeSubscription: async () => {},
    });
  return { subscriptions, sent, sendRaw, closed };
}

for (const testCase of fixture.busyRetry.filter((c) => c.busyCloses)) {
  test(`busyRetry: ${testCase.name}`, async () => {
    timers.clear();
    now = 0;
    const { subscriptions, sent, sendRaw, closed } = harness();
    const result = requestHistoryGated(
      subscriptions,
      sendRaw,
      async () => {},
      { kinds: [9], "#h": ["room-a"] },
      10 * 60 * 1000,
    );
    await flush();
    const delays = [];
    for (let close = 0; close < testCase.busyCloses; close += 1) {
      const last = sent.at(-1);
      closed(last.frame[1]);
      const before = sent.length;
      while (sent.length === before) {
        advance(1000);
        await flush();
      }
      delays.push((sent.at(-1).at - last.at) / 1000);
    }
    assert.deepEqual(delays, testCase.expect.delaysSecs);
    const ids = sent.map(({ frame }) => frame[1]);
    assert.equal(new Set(ids).size, ids.length, "a fresh sub id each retry");
    const last = ids.at(-1);
    handleSubscriptionEose({
      subscriptions,
      subId: last,
      closeSubscription: async () => {},
      generation: 1,
    });
    assert.deepEqual(await result, []);
  });
}

test("busyRetry: frames of the refused REQ after its CLOSED are not the retry's", async () => {
  timers.clear();
  now = 0;
  const { subscriptions, sent, sendRaw, closed } = harness();
  const result = requestHistoryGated(
    subscriptions,
    sendRaw,
    async () => {},
    { kinds: [9], "#h": ["room-a"] },
    60_000,
  );
  await flush();
  const refused = sent[0].frame[1];
  closed(refused);
  const late = { id: "late", kind: 9, created_at: 1, tags: [] };
  flushEvents(
    [{ subId: refused, event: late, generation: 1 }],
    subscriptions,
    1,
  );
  advance(1000);
  await flush();
  const retry = sent[1].frame[1];
  const x = { id: "x", kind: 9, created_at: 2, tags: [] };
  subscriptions.get(retry).events.push(x);
  handleSubscriptionEose({
    subscriptions,
    subId: retry,
    closeSubscription: async () => {},
    generation: 1,
  });
  assert.deepEqual(
    (await result).map((event) => event.id),
    ["x"],
  );
});

test("busyRetry: the caller's timeout still ends a re-keyed request", async () => {
  timers.clear();
  now = 0;
  const { subscriptions, sent, closed } = harness();
  const result = requestFirstEventGated(
    subscriptions,
    async (frame) => sent.push({ frame, at: now }),
    async () => {},
    { kinds: [0], authors: ["a"] },
    5_000,
  );
  await flush();
  closed(sent[0].frame[1]);
  advance(1000);
  await flush();
  closed(sent[1].frame[1]);
  advance(4000);
  await assert.rejects(result, /Timed out/);
  assert.equal(subscriptions.size, 0);
});
