import assert from "node:assert/strict";
import test from "node:test";

import { historyThenLive } from "./historyThenLive.ts";

const flush = async () => {
  for (let i = 0; i < 20; i++) await Promise.resolve();
};

function event(id, createdAt = 100) {
  return {
    id,
    pubkey: "a".repeat(64),
    kind: 44100,
    created_at: createdAt,
    tags: [],
    content: "",
    sig: "",
  };
}

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** Listen and fetch both wait for the test to release them. */
function rig() {
  const order = [];
  const applied = [];
  const listening = deferred();
  const fetching = [];
  let live = null;
  let stopped = 0;
  const options = {
    listen: (apply) => {
      order.push("listen");
      live = apply;
      return listening.promise.then(() => async () => {
        stopped += 1;
      });
    },
    fetch: () => {
      order.push("fetch");
      const next = deferred();
      fetching.push(next);
      return next.promise;
    },
    apply: (e) => applied.push(e.id),
  };
  return {
    order,
    applied,
    options,
    listening,
    fetching,
    emit: (e) => live(e),
    stopped: () => stopped,
  };
}

test("the fetch waits until the listener is live", async () => {
  const r = rig();
  const starting = historyThenLive(r.options);
  await flush();
  assert.deepEqual(r.order, ["listen"]);
  r.listening.resolve();
  const handle = await starting;
  assert.deepEqual(r.order, ["listen", "fetch"]);
  r.fetching[0].resolve([]);
  assert.equal(await handle.firstFetch, true);
});

test("an event live between ready and the fetch result is applied once, history oldest first", async () => {
  const r = rig();
  const starting = historyThenLive(r.options);
  r.listening.resolve();
  const handle = await starting;
  r.emit(event("gap", 300));
  r.fetching[0].resolve([
    event("new", 200),
    event("gap", 300),
    event("old", 100),
  ]);
  await handle.firstFetch;
  assert.deepEqual(r.applied, ["gap", "old", "new"]);
});

test("stop during the fetch applies nothing", async () => {
  const r = rig();
  const starting = historyThenLive(r.options);
  r.listening.resolve();
  const handle = await starting;
  await handle.stop();
  r.fetching[0].resolve([event("late")]);
  assert.equal(await handle.firstFetch, false);
  r.emit(event("after-stop"));
  assert.deepEqual(r.applied, []);
  assert.equal(r.stopped(), 1);
});

test("a fetch error reaches onFetchError and a refetch can still succeed", async () => {
  const r = rig();
  const errors = [];
  const starting = historyThenLive({
    ...r.options,
    onFetchError: (error) => errors.push(error.message),
  });
  r.listening.resolve();
  const handle = await starting;
  r.fetching[0].reject(new Error("timed out"));
  assert.equal(await handle.firstFetch, false);
  assert.deepEqual(errors, ["timed out"]);
  assert.deepEqual(r.applied, [], "a failure is not an empty success");

  const again = handle.refetch();
  await flush();
  r.fetching[1].resolve([event("e1")]);
  assert.equal(await again, true);
  assert.deepEqual(r.applied, ["e1"]);
});

test("a listen failure rejects without fetching", async () => {
  const r = rig();
  const starting = historyThenLive(r.options);
  r.listening.reject(new Error("closed"));
  await assert.rejects(starting, /closed/);
  assert.deepEqual(r.order, ["listen"]);
});
