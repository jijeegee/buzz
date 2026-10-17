import assert from "node:assert/strict";
import test from "node:test";

import {
  ChatStateWriter,
  MARK_READ_BATCH_MS,
  MARK_READ_MAX_ITEMS,
  markReadItemForContext,
  OWN_WRITE_PING_WINDOW_MS,
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

function harness({
  capability = "on",
  markRead,
  follow,
  isKnownNonMember,
} = {}) {
  const timers = fakeTimers();
  const calls = [];
  const unreadCalls = [];
  const followCalls = [];
  const written = [];
  let unavailable = 0;
  const clock = { now: 10_000 };
  const state = { capability };
  const writer = new ChatStateWriter({
    capability: () => state.capability,
    follow:
      follow ??
      (async (root, followState) => {
        followCalls.push([root, followState]);
        return { root, state: followState, thread_follow: 1 };
      }),
    isKnownNonMember,
    markRead:
      markRead ??
      (async (items, unreadMark) => {
        (unreadMark ? unreadCalls : calls).push(
          items.map((item) => ({ ...item })),
        );
        return { items: [], mark_read: 1 };
      }),
    now: () => clock.now,
    onUnavailable: () => {
      unavailable += 1;
      state.capability = "off";
    },
    onWritten: (write) => written.push(write),
    timers: timers.host,
  });
  return {
    calls,
    clock,
    followCalls,
    state,
    timers,
    unavailable: () => unavailable,
    unreadCalls,
    writer,
    written,
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

function refusingMarkRead(sent, refusedIds) {
  return async (items) => {
    sent.push(items.map((item) => item.id));
    const index = items.findIndex((item) => refusedIds.includes(item.id));
    if (index !== -1) {
      throw new Error(
        `relay returned 403 Forbidden: mark_read item ${index}: not a member of that room`,
      );
    }
    return { items: [], mark_read: 1 };
  };
}

test("a refused room is dropped, remembered for the session and never resent", async () => {
  const sent = [];
  const h = harness({ markRead: refusingMarkRead(sent, [DM]) });
  h.writer.queueRead(ROOM, 100);
  h.writer.queueRead(DM, 100);
  h.writer.queueRead(`thread:${ROOT}`, 100);
  await h.timers.advance(MARK_READ_BATCH_MS);
  assert.deepEqual(sent, [
    [ROOM, DM, ROOT],
    [ROOM, ROOT],
  ]);
  // Later reads and unread marks for the refused room are not sent.
  h.writer.queueRead(DM, 200);
  h.writer.queueUnread(DM);
  await h.timers.advance(MARK_READ_BATCH_MS * 3);
  assert.equal(sent.length, 2);
  assert.deepEqual(h.unreadCalls, []);
});

test("a second refusal defers the rest to the next batch instead of k+1 requests", async () => {
  const sent = [];
  const others = Array.from(
    { length: 4 },
    (_, i) => `${i}a1657ac-f7aa-5db0-b632-d8bbeb6dfb50`,
  );
  const h = harness({ markRead: refusingMarkRead(sent, others.slice(0, 3)) });
  for (const id of others) h.writer.queueRead(id, 100);
  h.writer.queueRead(ROOM, 100);
  await h.timers.advance(MARK_READ_BATCH_MS);
  // First try, then one retry without the first refused room: two requests.
  assert.equal(sent.length, 2);
  assert.deepEqual(sent[1], [others[1], others[2], others[3], ROOM]);
  await h.timers.advance(MARK_READ_BATCH_MS);
  assert.deepEqual(sent[2], [others[2], others[3], ROOM]);
  assert.deepEqual(sent[3], [others[3], ROOM]);
  await h.timers.advance(MARK_READ_BATCH_MS * 5);
  assert.equal(sent.length, 4);
});

test("rooms known not to be joined are never mirrored", async () => {
  const h = harness({ isKnownNonMember: (item) => item.id === DM });
  h.writer.queueRead(DM, 100);
  h.writer.queueRead(ROOM, 100);
  await h.timers.advance(MARK_READ_BATCH_MS);
  assert.deepEqual(h.calls, [[{ at: 100, id: ROOM, kind: "channel" }]]);
});

test("another account or relay drops the queue and stops the writer", async () => {
  let attempts = 0;
  const h = harness({
    markRead: async () => {
      attempts += 1;
      throw new Error("chat_state_account_changed");
    },
  });
  h.writer.queueRead(ROOM, 100);
  h.writer.queueRead(DM, 100);
  await h.timers.advance(MARK_READ_BATCH_MS);
  assert.equal(attempts, 1);
  assert.equal(h.writer.isDisposed, true);
  // Not "permanent", not retried, and nothing more is ever sent.
  h.writer.queueRead(ROOM, 300);
  h.writer.setFollow(ROOT, "followed");
  await h.writer.flush();
  await h.timers.advance(MARK_READ_BATCH_MS * 100);
  assert.equal(attempts, 1);
  assert.deepEqual(h.followCalls, []);
  assert.deepEqual(h.timers.pending(), []);
  assert.equal(h.unavailable(), 0);
});

test("dispose (account switch, reload) drops pending writes without sending", async () => {
  const h = harness();
  h.writer.queueRead(ROOM, 100);
  h.writer.queueUnread(DM);
  h.writer.setFollow(ROOT, "followed");
  h.writer.dispose();
  await h.writer.flush();
  await h.timers.advance(MARK_READ_BATCH_MS * 5);
  assert.deepEqual(h.calls, []);
  assert.deepEqual(h.unreadCalls, []);
  assert.deepEqual(h.followCalls, []);
});

test("a flush sends every chunk, including reads queued while it ran", async () => {
  let release;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  const sizes = [];
  const h = harness({
    markRead: async (items) => {
      sizes.push(items.length);
      if (sizes.length === 1) await gate;
      return { items: [], mark_read: 1 };
    },
  });
  for (let i = 0; i < MARK_READ_MAX_ITEMS * 2 + 3; i += 1) {
    h.writer.queueRead(`thread:${i.toString(16).padStart(64, "0")}`, 10);
  }
  const flushed = h.writer.flush();
  await Promise.resolve();
  h.writer.queueRead(ROOM, 50);
  release();
  await flushed;
  assert.deepEqual(sizes, [MARK_READ_MAX_ITEMS, MARK_READ_MAX_ITEMS, 4]);
});

test("unread marks go out after reads with unread_mark; a later read clears them", async () => {
  const h = harness();
  h.writer.queueRead(ROOM, 100);
  h.writer.queueUnread(ROOM);
  h.writer.queueUnread(`thread:${ROOT}`);
  h.writer.queueUnread(`msg:${ROOT}`);
  await h.timers.advance(MARK_READ_BATCH_MS);
  // The mark replaced the pending read for the same room.
  assert.deepEqual(h.calls, []);
  assert.deepEqual(h.unreadCalls, [
    [
      { id: ROOM, kind: "channel" },
      { id: ROOT, kind: "thread" },
    ],
  ]);
  // Reading again, even at an already-sent position, clears the mark.
  h.writer.queueRead(ROOM, 100);
  await h.timers.advance(MARK_READ_BATCH_MS);
  assert.deepEqual(h.calls, [[{ at: 100, id: ROOM, kind: "channel" }]]);
});

test("pings during or just after this device's own write are its echo", async () => {
  let release;
  const h = harness({
    markRead: (items) =>
      new Promise((resolve) => {
        release = () => resolve({ items, mark_read: 1 });
      }),
  });
  assert.equal(h.writer.explainsPing(), false);
  h.writer.queueRead(ROOM, 100);
  await h.timers.advance(MARK_READ_BATCH_MS);
  // The relay pings before it answers.
  assert.equal(h.writer.explainsPing(), true);
  release();
  for (let i = 0; i < 10; i += 1) await Promise.resolve();
  assert.equal(h.written.length, 1);
  assert.equal(h.written[0].type, "mark_read");
  h.clock.now += OWN_WRITE_PING_WINDOW_MS - 1;
  assert.equal(h.writer.explainsPing(), true);
  h.clock.now += 1;
  assert.equal(h.writer.explainsPing(), false);
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

test("after forgetSent, an explicit read at an already sent position is sent again", async () => {
  const h = harness();
  h.writer.queueRead(`thread:${ROOT}`, 100);
  await h.timers.advance(MARK_READ_BATCH_MS);
  assert.equal(h.calls.length, 1);
  // Same position again: deduped.
  h.writer.queueRead(`thread:${ROOT}`, 100);
  await h.timers.advance(MARK_READ_BATCH_MS * 3);
  assert.equal(h.calls.length, 1);
  // Another device marked it unread; the row's Mark as read resends.
  h.writer.forgetSent(`thread:${ROOT}`);
  h.writer.queueRead(`thread:${ROOT}`, 100);
  await h.timers.advance(MARK_READ_BATCH_MS * 3);
  assert.equal(h.calls.length, 2);
  assert.deepEqual(h.calls[1], [{ at: 100, id: ROOT, kind: "thread" }]);
});
