import assert from "node:assert/strict";
import test from "node:test";

import {
  WATCHING_INTERVAL_MS,
  createObserverWatchingRegistry,
} from "./observerWatchingRegistry.ts";

const AGENT = "AB".repeat(32);
const OTHER = "cd".repeat(32);

/** A registry on a manual clock and a manual relay connection. */
function harness({ connected = true } = {}) {
  const sent = [];
  const timers = new Map();
  let nextTimer = 1;
  let now = 0;
  const connection = { connected, listeners: new Set() };
  const registry = createObserverWatchingRegistry({
    send: async (agentPubkey, channelId) => {
      sent.push({ at: now, agentPubkey, channelId });
    },
    isConnected: () => connection.connected,
    subscribeConnection: (listener) => {
      connection.listeners.add(listener);
      return () => connection.listeners.delete(listener);
    },
    setInterval: (callback, ms) => {
      const id = nextTimer++;
      timers.set(id, { callback, ms, next: now + ms });
      return id;
    },
    clearInterval: (id) => timers.delete(id),
  });
  return {
    registry,
    sent,
    timers,
    advance(ms) {
      const end = now + ms;
      for (;;) {
        const due = [...timers.values()]
          .filter((timer) => timer.next <= end)
          .sort((a, b) => a.next - b.next)[0];
        if (!due) break;
        now = due.next;
        due.next += due.ms;
        due.callback();
      }
      now = end;
    },
    setConnected(next) {
      connection.connected = next;
      for (const listener of connection.listeners) listener(next);
    },
    listenerCount: () => connection.listeners.size,
  };
}

test("opening a live view sends watching immediately, then every 30 s", () => {
  const h = harness();
  const release = h.registry.acquire(AGENT, "channel-1");
  assert.deepEqual(h.sent, [
    { at: 0, agentPubkey: AGENT.toLowerCase(), channelId: "channel-1" },
  ]);
  h.advance(WATCHING_INTERVAL_MS - 1);
  assert.equal(h.sent.length, 1);
  h.advance(1);
  assert.equal(h.sent.length, 2);
  h.advance(WATCHING_INTERVAL_MS * 3);
  assert.deepEqual(
    h.sent.map((entry) => entry.at),
    [0, 30_000, 60_000, 90_000, 120_000],
  );
  release();
});

test("closing the view stops the timer and sends nothing more", () => {
  const h = harness();
  const release = h.registry.acquire(AGENT, "channel-1");
  h.advance(WATCHING_INTERVAL_MS);
  release();
  release();
  h.advance(WATCHING_INTERVAL_MS * 4);
  assert.equal(h.sent.length, 2);
  assert.equal(h.timers.size, 0);
  assert.deepEqual(h.registry.heldAgents(), []);
  assert.equal(h.listenerCount(), 0, "idle registry drops its listener");
});

test("two live panels for one agent share one sender", () => {
  const h = harness();
  const releaseThread = h.registry.acquire(AGENT, "channel-1");
  h.advance(10_000);
  const releaseProfile = h.registry.acquire(AGENT.toLowerCase(), "channel-2");
  assert.equal(h.sent.length, 1, "the second panel does not send again");
  assert.equal(h.timers.size, 1);
  h.advance(WATCHING_INTERVAL_MS * 2);
  assert.equal(h.sent.length, 3, "one frame per 30 s, not two");
  assert.equal(h.sent.at(-1).channelId, "channel-2", "newest view names it");

  releaseThread();
  h.advance(WATCHING_INTERVAL_MS);
  assert.equal(h.sent.length, 4, "still watched by the remaining panel");
  releaseProfile();
  h.advance(WATCHING_INTERVAL_MS);
  assert.equal(h.sent.length, 4);
});

test("different agents each get their own sender", () => {
  const h = harness();
  const a = h.registry.acquire(AGENT, "channel-1");
  const b = h.registry.acquire(OTHER, "channel-1");
  h.advance(WATCHING_INTERVAL_MS);
  assert.equal(h.sent.length, 4);
  assert.equal(h.timers.size, 2);
  a();
  b();
});

test("pauses while disconnected and sends at once on reconnect", () => {
  const h = harness();
  const release = h.registry.acquire(AGENT, "channel-1");
  h.advance(5_000);
  h.setConnected(false);
  assert.equal(h.timers.size, 0, "no timer while disconnected");
  h.advance(WATCHING_INTERVAL_MS * 3);
  assert.equal(h.sent.length, 1);

  h.setConnected(true);
  assert.equal(h.sent.length, 2, "immediate send on reconnect");
  assert.equal(h.sent[1].at, 95_000);
  h.advance(WATCHING_INTERVAL_MS);
  assert.deepEqual(
    h.sent.map((entry) => entry.at),
    [0, 95_000, 125_000],
    "the timer restarts from the reconnect",
  );
  release();
});

test("a view opened while disconnected waits for the connection", () => {
  const h = harness({ connected: false });
  const release = h.registry.acquire(AGENT, "channel-1");
  h.advance(WATCHING_INTERVAL_MS * 2);
  assert.equal(h.sent.length, 0);
  h.setConnected(true);
  assert.equal(h.sent.length, 1);
  release();
  h.setConnected(false);
  h.setConnected(true);
  assert.equal(h.sent.length, 1, "released agents are not resent");
});

test("a failed send is reported and the next tick retries", async () => {
  const errors = [];
  let attempts = 0;
  const intervals = [];
  const registry = createObserverWatchingRegistry({
    send: async () => {
      attempts += 1;
      throw new Error("relay rejected");
    },
    isConnected: () => true,
    subscribeConnection: () => () => {},
    setInterval: (callback) => {
      intervals.push(callback);
      return intervals.length;
    },
    clearInterval: () => {},
    onSendError: (error) => errors.push(error.message),
  });
  const release = registry.acquire(AGENT, null);
  intervals[0]();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(attempts, 2);
  assert.deepEqual(errors, ["relay rejected", "relay rejected"]);
  release();
});
