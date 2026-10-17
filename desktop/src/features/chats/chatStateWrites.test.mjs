import assert from "node:assert/strict";
import test from "node:test";

import {
  ChatStateWriter,
  MARK_READ_BATCH_MS,
  MARK_READ_MAX_ITEMS,
  markReadItemForContext,
} from "./chatStateWrites.ts";

const ROOM = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
const DM = "8ad375a7-6990-4b22-985f-e3fd34f634d7";
const ROOT = "c".repeat(64);

function fakeTimers() {
  let now = 0;
  let nextId = 1;
  const timers = new Map();
  return {
    host: {
      setTimeout(handler, ms) {
        const id = nextId++;
        timers.set(id, { at: now + ms, handler });
        return id;
      },
      clearTimeout(id) {
        timers.delete(id);
      },
    },
    pending: () => [...timers.values()].map((timer) => timer.at - now),
    async advance(ms) {
      now += ms;
      for (const [id, timer] of [...timers]) {
        if (timer.at <= now) {
          timers.delete(id);
          timer.handler();
        }
      }
      // Let the request promise chains settle.
      for (let i = 0; i < 20; i += 1) await Promise.resolve();
    },
  };
}

function harness({ capability = "on", markRead, follow } = {}) {
  const timers = fakeTimers();
  const calls = [];
  const followCalls = [];
  let unavailable = 0;
  const state = { capability };
  const writer = new ChatStateWriter({
    capability: () => state.capability,
    follow:
      follow ??
      (async (root, followState) => {
        followCalls.push([root, followState]);
      }),
    markRead:
      markRead ??
      (async (items) => {
        calls.push(items.map((item) => ({ ...item })));
      }),
    onUnavailable: () => {
      unavailable += 1;
      state.capability = "off";
    },
    timers: timers.host,
  });
  return {
    calls,
    followCalls,
    state,
    timers,
    unavailable: () => unavailable,
    writer,
  };
}

test("contexts map to mark_read items; msg: and malformed ids are skipped", () => {
  assert.deepEqual(markReadItemForContext(ROOM, 1_760_000_000.7), {
    at: 1_760_000_000,
    id: ROOM,
    kind: "channel",
  });
  assert.deepEqual(markReadItemForContext(`thread:${ROOT.toUpperCase()}`, 5), {
    at: 5,
    id: ROOT,
    kind: "thread",
  });
  assert.equal(markReadItemForContext(`msg:${ROOT}`, 5), null);
  assert.equal(markReadItemForContext("thread:abc", 5), null);
  assert.equal(markReadItemForContext("not-a-uuid", 5), null);
  assert.equal(markReadItemForContext(ROOM, 0), null);
});

test("reads merge per context and go out as one batch after the debounce", async () => {
  const h = harness();
  h.writer.queueRead(ROOM, 100);
  h.writer.queueRead(ROOM, 90);
  h.writer.queueRead(ROOM, 120);
  h.writer.queueRead(`thread:${ROOT}`, 110);
  h.writer.queueRead(`msg:${ROOT}`, 110);
  await h.timers.advance(MARK_READ_BATCH_MS - 1);
  assert.equal(h.calls.length, 0);
  await h.timers.advance(1);
  assert.deepEqual(h.calls, [
    [
      { at: 120, id: ROOM, kind: "channel" },
      { at: 110, id: ROOT, kind: "thread" },
    ],
  ]);
  // Already-sent positions are not sent again; newer ones are.
  h.writer.queueRead(ROOM, 120);
  await h.timers.advance(MARK_READ_BATCH_MS);
  assert.equal(h.calls.length, 1);
  h.writer.queueRead(ROOM, 121);
  await h.timers.advance(MARK_READ_BATCH_MS);
  assert.deepEqual(h.calls[1], [{ at: 121, id: ROOM, kind: "channel" }]);
});

test("large batches split at the relay's item limit", async () => {
  const h = harness();
  for (let i = 0; i < MARK_READ_MAX_ITEMS + 5; i += 1) {
    h.writer.queueRead(`thread:${i.toString(16).padStart(64, "0")}`, 10);
  }
  await h.timers.advance(MARK_READ_BATCH_MS);
  await h.timers.advance(MARK_READ_BATCH_MS);
  assert.deepEqual(
    h.calls.map((items) => items.length),
    [MARK_READ_MAX_ITEMS, 5],
  );
});

test("hiding the app flushes at once", async () => {
  const h = harness();
  h.writer.queueRead(ROOM, 100);
  await h.writer.flush();
  assert.equal(h.calls.length, 1);
  assert.deepEqual(h.timers.pending(), []);
});

test("unknown capability buffers; on sends; off drops", async () => {
  const h = harness({ capability: "unknown" });
  h.writer.queueRead(ROOM, 100);
  h.writer.setFollow(ROOT, "followed");
  await h.timers.advance(MARK_READ_BATCH_MS * 5);
  assert.equal(h.calls.length, 0);
  assert.equal(h.followCalls.length, 0);
  h.state.capability = "on";
  h.writer.capabilityChanged();
  await h.timers.advance(MARK_READ_BATCH_MS);
  assert.equal(h.calls.length, 1);
  assert.deepEqual(h.followCalls, [[ROOT, "followed"]]);

  const off = harness({ capability: "unknown" });
  off.writer.queueRead(ROOM, 100);
  off.state.capability = "off";
  off.writer.capabilityChanged();
  off.writer.queueRead(DM, 100);
  off.state.capability = "on";
  off.writer.capabilityChanged();
  await off.timers.advance(MARK_READ_BATCH_MS * 2);
  assert.equal(off.calls.length, 0);
});

test("unavailable turns the capability off and drops the queue", async () => {
  let attempts = 0;
  const h = harness({
    markRead: async () => {
      attempts += 1;
      throw new Error("chat_state_unavailable");
    },
  });
  h.writer.queueRead(ROOM, 100);
  await h.timers.advance(MARK_READ_BATCH_MS);
  assert.equal(h.unavailable(), 1);
  assert.equal(h.state.capability, "off");
  h.writer.queueRead(DM, 200);
  await h.timers.advance(MARK_READ_BATCH_MS * 10);
  assert.equal(attempts, 1);
  assert.deepEqual(h.timers.pending(), []);
});

test("a refused item (not a member) is dropped and the rest resent", async () => {
  const sent = [];
  const h = harness({
    markRead: async (items) => {
      sent.push(items.map((item) => item.id));
      const index = items.findIndex((item) => item.id === DM);
      if (index !== -1) {
        throw new Error(
          `relay returned 403 Forbidden: mark_read item ${index}: not a member of that room`,
        );
      }
    },
  });
  h.writer.queueRead(ROOM, 100);
  h.writer.queueRead(DM, 100);
  h.writer.queueRead(`thread:${ROOT}`, 100);
  await h.timers.advance(MARK_READ_BATCH_MS);
  assert.deepEqual(sent, [
    [ROOM, DM, ROOT],
    [ROOM, ROOT],
  ]);
  // The refused context may be tried again with a later position.
  h.writer.queueRead(DM, 100);
  await h.timers.advance(MARK_READ_BATCH_MS);
  assert.deepEqual(sent[2], [DM]);
});

test("malformed requests are dropped; network failures retry with backoff", async () => {
  const bad = harness({
    markRead: async () => {
      throw new Error(
        "relay returned 400 Bad Request: mark_read item 0: accepts only channel, thread, seq, event",
      );
    },
  });
  bad.writer.queueRead(ROOM, 100);
  await bad.timers.advance(MARK_READ_BATCH_MS);
  assert.deepEqual(bad.timers.pending(), []);

  let fail = true;
  const sent = [];
  const flaky = harness({
    markRead: async (items) => {
      if (fail) throw new Error("relay unreachable: request timed out");
      sent.push(items.map((item) => item.at));
    },
  });
  flaky.writer.queueRead(ROOM, 100);
  await flaky.timers.advance(MARK_READ_BATCH_MS);
  assert.deepEqual(flaky.timers.pending(), [MARK_READ_BATCH_MS * 2]);
  // A newer read while waiting merges into the retry.
  flaky.writer.queueRead(ROOM, 130);
  fail = false;
  await flaky.timers.advance(MARK_READ_BATCH_MS * 2);
  assert.deepEqual(sent, [[130]]);
  assert.deepEqual(flaky.timers.pending(), []);
});

test("follow choices go out at once, newest per thread, and drop when refused", async () => {
  const h = harness();
  h.writer.setFollow(ROOT, "followed");
  h.writer.setFollow(ROOT, "unfollowed");
  h.writer.setFollow("not-hex", "followed");
  await h.timers.advance(0);
  assert.deepEqual(h.followCalls, [[ROOT, "unfollowed"]]);

  let calls = 0;
  const refused = harness({
    follow: async () => {
      calls += 1;
      throw new Error(
        "relay returned 403 Forbidden: thread_follow: not a member of that room",
      );
    },
  });
  refused.writer.setFollow(ROOT, "followed");
  await refused.timers.advance(0);
  await refused.timers.advance(MARK_READ_BATCH_MS * 100);
  assert.equal(calls, 1);
});
