import assert from "node:assert/strict";
import { test } from "node:test";

import {
  keepLiveSubscription,
  LIVE_SUBSCRIPTION_MAX_ATTEMPTS,
} from "./liveSubscriptionRetry.ts";

function fakeTimers() {
  const pending = new Map();
  let next = 1;
  return {
    setTimeout(fn) {
      const id = next++;
      pending.set(id, fn);
      return id;
    },
    clearTimeout(id) {
      pending.delete(id);
    },
    async runAll() {
      for (const [id, fn] of [...pending]) {
        pending.delete(id);
        fn();
      }
      await flush();
    },
    get size() {
      return pending.size;
    },
  };
}

const flush = async () => {
  for (let i = 0; i < 10; i++) await Promise.resolve();
};

test("a failed open is retried until it opens", async () => {
  const timers = fakeTimers();
  let calls = 0;
  const opened = [];
  keepLiveSubscription({
    open: async () => {
      calls += 1;
      if (calls < 3) throw new Error("socket not ready");
      return async () => {};
    },
    onReconnect: () => () => {},
    onOpened: () => opened.push(calls),
    timers,
  });
  await flush();
  await timers.runAll();
  await timers.runAll();
  assert.equal(calls, 3);
  assert.deepEqual(opened, [3]);
  assert.equal(timers.size, 0);
});

test("a lasting failure stops after its attempts and resumes on reconnect", async () => {
  const timers = fakeTimers();
  let calls = 0;
  let reconnect = () => {};
  keepLiveSubscription({
    open: async () => {
      calls += 1;
      throw new Error("down");
    },
    onReconnect: (listener) => {
      reconnect = listener;
      return () => {};
    },
    timers,
  });
  await flush();
  for (let i = 0; i < LIVE_SUBSCRIPTION_MAX_ATTEMPTS + 2; i++) {
    await timers.runAll();
  }
  assert.equal(calls, LIVE_SUBSCRIPTION_MAX_ATTEMPTS);
  assert.equal(timers.size, 0);
  reconnect();
  await flush();
  assert.equal(calls, LIVE_SUBSCRIPTION_MAX_ATTEMPTS + 1);
});

test("dispose cancels a pending retry and closes an open subscription", async () => {
  const timers = fakeTimers();
  let fail = true;
  let closed = 0;
  const dispose = keepLiveSubscription({
    open: async () => {
      if (fail) throw new Error("down");
      return async () => {
        closed += 1;
      };
    },
    onReconnect: () => () => {},
    timers,
  });
  await flush();
  assert.equal(timers.size, 1);
  dispose();
  assert.equal(timers.size, 0);

  fail = false;
  const disposeOpen = keepLiveSubscription({
    open: async () => async () => {
      closed += 1;
    },
    onReconnect: () => () => {},
    timers,
  });
  await flush();
  disposeOpen();
  assert.equal(closed, 1);
});
