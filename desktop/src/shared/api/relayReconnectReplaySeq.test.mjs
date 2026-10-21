// Order-number (after_seq) repair for connections to a kuzz-seq relay.
import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";

globalThis.window = {
  setTimeout: (fn) => {
    fn();
    return 1;
  },
  clearTimeout() {},
};

const { replayLiveSubscriptions, buildLiveOnlyFilter } = await import(
  "./relayReconnectReplay.ts"
);
const { buildChannelFilter } = await import("./relayChannelFilters.ts");
const { SeqCursors } = await import("./relaySeqCursor.ts");
const { resetRateLimitGate } = await import("./relayRateLimitGate.ts");

const CH = "270f6caf-0feb-4055-93f3-cdbeb567ff28";
const OTHER = "11111111-1111-4111-8111-111111111111";

beforeEach(() => resetRateLimitGate());

function event(id, createdAt) {
  return {
    id,
    pubkey: "p",
    created_at: createdAt,
    kind: 9,
    tags: [["h", CH]],
    content: "",
    sig: "s",
  };
}

function boundsEvent(afterSeq, lastSeq, hasMore, sequences) {
  return {
    ...event(`bounds-${afterSeq}`, 0),
    kind: 39006,
    tags: [["d", `${CH}:after_seq:${afterSeq}`]],
    content: JSON.stringify({
      order: "seq",
      after_seq: afterSeq,
      last_seq: lastSeq,
      has_more: hasMore,
      sequences,
    }),
  };
}

function liveSub(channelId = CH, extra = {}) {
  const received = [];
  return {
    received,
    subscription: {
      mode: "live",
      filter: {
        ...buildChannelFilter(channelId, 1000),
        since: 1_000,
      },
      onEvent: (e) => received.push(e),
      lastSeenCreatedAt: 2_000,
      ...extra,
    },
  };
}

function cursorsAt(channelId, ...seqs) {
  const cursors = new SeqCursors();
  for (const seq of seqs) cursors.observe(channelId, seq, 0);
  return cursors;
}

test("opted-in reconnect sends live-only REQs and pages after_seq once per channel", async () => {
  const a = liveSub();
  const b = liveSub();
  const subscriptions = new Map([
    ["a", a.subscription],
    ["b", b.subscription],
  ]);
  const sent = [];
  const seqRequests = [];
  const cursors = cursorsAt(CH, 10, 11);
  await replayLiveSubscriptions({
    subscriptions,
    sendRaw: async (payload) => sent.push(payload),
    requestRepair: async () => assert.fail("time repair must not run"),
    seq: {
      cursors,
      now: () => 0,
      requestSeqRepair: async (request) => {
        seqRequests.push(request);
        if (request.afterSeq === 9) {
          return [
            event("m10", 50),
            event("m12", 40),
            boundsEvent(9, 12, true, [
              ["m10", 10],
              ["m12", 12],
            ]),
          ];
        }
        return [event("m13", 60), boundsEvent(12, 13, false, [["m13", 13]])];
      },
    },
  });

  assert.deepEqual(
    sent.map(([type, subId, filter]) => [type, subId, filter]),
    [
      ["REQ", "a", buildLiveOnlyFilter(a.subscription.filter)],
      ["REQ", "b", buildLiveOnlyFilter(b.subscription.filter)],
    ],
  );
  assert.equal(sent[0][2].since, undefined);
  assert.equal(sent[0][2].limit, 0);
  assert.deepEqual(seqRequests, [
    { channelId: CH, afterSeq: 9, limit: 500 },
    { channelId: CH, afterSeq: 12, limit: 500 },
  ]);
  for (const sub of [a, b]) {
    assert.deepEqual(
      sub.received.map((e) => [e.id, e.seq]),
      [
        ["m10", 10],
        ["m12", 12],
        ["m13", 13],
      ],
      "bounds events are never dispatched",
    );
  }
  assert.equal(a.subscription.reconnectReplay?.repairDone ?? true, true);
});

test("a channel without a known number keeps the time repair path", async () => {
  const a = liveSub(OTHER);
  const subscriptions = new Map([["a", a.subscription]]);
  const sent = [];
  const timeRequests = [];
  await replayLiveSubscriptions({
    subscriptions,
    sendRaw: async (payload) => sent.push(payload),
    requestRepair: async (request) => {
      timeRequests.push(request);
      return [];
    },
    seq: {
      cursors: cursorsAt(CH, 5),
      now: () => 0,
      requestSeqRepair: async () => assert.fail("no number for this channel"),
    },
  });
  assert.equal(sent[0][2].since, a.subscription.filter.since);
  assert.equal(sent[0][2].limit, 1000);
  assert.equal(timeRequests.length, 1);
  assert.equal(timeRequests[0].channelId, OTHER);
});

test("without kuzz-seq the replay is unchanged even when numbers are known", async () => {
  const a = liveSub();
  const sent = [];
  let timeRequests = 0;
  await replayLiveSubscriptions({
    subscriptions: new Map([["a", a.subscription]]),
    sendRaw: async (payload) => sent.push(payload),
    requestRepair: async () => {
      timeRequests += 1;
      return [];
    },
  });
  assert.deepEqual(sent[0][2], a.subscription.filter);
  assert.equal(timeRequests, 1);
});

test("a repair on open REQs sends nothing and only repairs by number", async () => {
  const a = liveSub();
  const other = liveSub(OTHER);
  const sent = [];
  const seqRequests = [];
  await replayLiveSubscriptions({
    subscriptions: new Map([
      ["a", a.subscription],
      ["other", other.subscription],
    ]),
    sendRaw: async (payload) => sent.push(payload),
    requestRepair: async () => assert.fail("no time repair on open REQs"),
    resendLive: false,
    seq: {
      cursors: cursorsAt(CH, 20),
      now: () => 0,
      requestSeqRepair: async (request) => {
        seqRequests.push(request.afterSeq);
        return [event("m21", 1), boundsEvent(19, 21, false, [["m21", 21]])];
      },
    },
  });
  assert.deepEqual(sent, []);
  assert.deepEqual(seqRequests, [19]);
  assert.deepEqual(
    a.received.map((e) => e.id),
    ["m21"],
  );
  assert.deepEqual(other.received, []);
  assert.equal(a.subscription.reconnectReplay, undefined, "marker released");
});

test("a failed pass pins its start so the next repair does not skip it", async () => {
  const a = liveSub();
  const cursors = cursorsAt(CH, 30);
  let calls = 0;
  await replayLiveSubscriptions({
    subscriptions: new Map([["a", a.subscription]]),
    sendRaw: async () => {},
    requestRepair: async () => [],
    resendLive: false,
    seq: {
      cursors,
      now: () => 0,
      requestSeqRepair: async () => {
        calls += 1;
        throw new Error("rate-limited: slow down");
      },
    },
  });
  assert.equal(calls, 3, "bounded retries");
  // Live numbers keep arriving after the failure.
  cursors.observe(CH, 90, 100_000);
  cursors.observe(CH, 95, 200_000);
  assert.equal(cursors.replayPoint(CH, 300_000), 29);
});

test("a page without a bounds event is an error, not a silent completion", async () => {
  const a = liveSub();
  const errors = [];
  const originalWarn = console.warn;
  console.warn = (...args) => errors.push(args);
  try {
    await replayLiveSubscriptions({
      subscriptions: new Map([["a", a.subscription]]),
      sendRaw: async () => {},
      requestRepair: async () => [],
      resendLive: false,
      seq: {
        cursors: cursorsAt(CH, 3),
        now: () => 0,
        requestSeqRepair: async () => [event("x", 1)],
      },
    });
  } finally {
    console.warn = originalWarn;
  }
  assert.equal(errors.length, 3);
  assert.deepEqual(a.received, [], "rows without numbers are not trusted");
});
