import assert from "node:assert/strict";
import test from "node:test";

import { CHANNEL_EVENT_KINDS } from "@/shared/constants/kinds";
import { retireLiveSubscription } from "./relayClosedRecovery.ts";
import { RECONNECT_REPLAY_CHANNEL_LOOKBACK_SECS } from "./relayReconnectReplay.ts";
import {
  catchUpFromStore,
  noteStoreLiveEose,
  noteStoreLiveEvent,
  noteStoreRepairEnded,
  setStoreCoverageDeps,
  storeLiveAt,
  STORE_CATCH_UP_MAX_PAGES,
  STORE_CATCH_UP_PAGE_LIMIT,
  STORE_RESUME_LOOKBACK_SECS,
  waitStoreLive,
} from "./relayStoreCoverage.ts";

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

function harness({ now = 10_000, points = {}, pages = [] } = {}) {
  const state = { now, extended: [], requests: [], scope: "s1", scopes: [] };
  const restore = setStoreCoverageDeps({
    nowSecs: () => state.now,
    scope: async () => state.scope,
    extend: async (roomIds, from, to, scope) => {
      state.extended.push([roomIds, from, to]);
      state.scopes.push(scope);
    },
    resumePoints: async (_roomIds, before) => {
      state.resumeBefore = before;
      return points;
    },
    requestPage: async (request) => {
      state.requests.push(request);
      return pages.shift() ?? [];
    },
  });
  return { state, restore };
}

const bundle = (rooms = ["r1", "r2"]) => ({
  mode: "live",
  filter: { kinds: [...CHANNEL_EVENT_KINDS], "#h": rooms, limit: 0 },
  onEvent: () => {},
});

const page = (count, createdAt) =>
  Array.from({ length: count }, (_, i) => ({
    id: String(i).padStart(64, "0"),
    created_at: createdAt,
  }));

test("resume margin matches reconnect repair", () => {
  assert.equal(
    STORE_RESUME_LOOKBACK_SECS,
    RECONNECT_REPLAY_CHANNEL_LOOKBACK_SECS,
  );
});

test("EOSE opens coverage and catches up stored rooms only", async () => {
  const { state, restore } = harness({ points: { r1: 5_000 } });
  try {
    noteStoreLiveEose(bundle(), 1);
    await tick();
    assert.deepEqual(state.requests[0].channelIds, ["r1"]);
    assert.equal(state.requests[0].since, 5_000 - STORE_RESUME_LOOKBACK_SECS);
    // The first page is not cut at this PC's clock.
    assert.equal(state.requests[0].until, undefined);
    assert.deepEqual(state.extended, [
      [["r1", "r2"], 10_000, 10_000],
      [["r1"], 5_000 - STORE_RESUME_LOOKBACK_SECS, 10_000],
    ]);
  } finally {
    restore();
  }
});

test("a reconnect repair on this connection skips the store catch-up", async () => {
  const { state, restore } = harness({ points: { r1: 5_000 } });
  try {
    const subscription = bundle();
    subscription.reconnectReplay = { generation: 2 };
    noteStoreLiveEose(subscription, 2);
    await tick();
    assert.equal(state.requests.length, 0);
  } finally {
    restore();
  }
});

test("events extend coverage at most once a minute, on the same connection", async () => {
  const { state, restore } = harness();
  try {
    const subscription = bundle();
    noteStoreLiveEose(subscription, 1);
    state.now += 30;
    noteStoreLiveEvent(subscription, 1);
    state.now += 40;
    noteStoreLiveEvent(subscription, 2);
    noteStoreLiveEvent(subscription, 1);
    await tick();
    assert.deepEqual(state.extended.slice(1), [[["r1", "r2"], 10_000, 10_070]]);
  } finally {
    restore();
  }
});

test("a gap larger than the page cap is not recorded", async () => {
  const pages = Array.from({ length: STORE_CATCH_UP_MAX_PAGES }, (_, i) =>
    page(STORE_CATCH_UP_PAGE_LIMIT, 9_000 - i),
  );
  const { state, restore } = harness({ points: { r1: 5_000 }, pages });
  try {
    assert.equal(await catchUpFromStore(["r1"], 10_000), false);
    assert.equal(state.requests.length, STORE_CATCH_UP_MAX_PAGES);
    // The REQ's own span, opened at liveAt, is not a resume point.
    assert.equal(state.resumeBefore, 10_000);
    assert.equal(state.extended.length, 0);
  } finally {
    restore();
  }
});

test("a gap older than a week is left to opening the room", async () => {
  const { state, restore } = harness({ points: { r1: 1 } });
  try {
    assert.equal(await catchUpFromStore(["r1"], 10 * 24 * 60 * 60), false);
    assert.equal(state.requests.length, 0);
  } finally {
    restore();
  }
});

test("a room is live from its REQ's EOSE until the REQ is retired", () => {
  const { state, restore } = harness();
  try {
    const subscription = bundle(["x1", "x2"]);
    assert.equal(storeLiveAt("x1"), null);
    noteStoreLiveEose(subscription, 1);
    assert.equal(storeLiveAt("x1"), 10_000);
    assert.equal(storeLiveAt("x3"), null);
    // A second chunk carrying the room since later does not move it.
    state.now += 50;
    noteStoreLiveEose(bundle(["x1"]), 1);
    assert.equal(storeLiveAt("x1"), 10_000);
    retireLiveSubscription(subscription);
    assert.equal(storeLiveAt("x1"), 10_050);
  } finally {
    restore();
  }
});

test("spans carry the scope of their EOSE, and a retired REQ stops its catch-up", async () => {
  const gate = Promise.withResolvers();
  const { state, restore } = harness({ points: { r1: 5_000 } });
  setStoreCoverageDeps({
    requestPage: async (request) => {
      state.requests.push(request);
      await gate.promise;
      return page(1, 9_000);
    },
  });
  try {
    const subscription = bundle();
    noteStoreLiveEose(subscription, 1);
    await tick();
    assert.equal(state.requests.length, 1);
    // The account switches; the REQ is retired while its walk is in flight.
    state.scope = "s2";
    retireLiveSubscription(subscription);
    gate.resolve();
    await tick();
    await tick();
    assert.deepEqual(state.extended, [[["r1", "r2"], 10_000, 10_000]]);
    assert.deepEqual(state.scopes, ["s1"]);
  } finally {
    restore();
  }
});

test("with no store scope (signed out) nothing is recorded", async () => {
  const { state, restore } = harness({ points: { r1: 5_000 } });
  state.scope = null;
  try {
    noteStoreLiveEose(bundle(), 1);
    await tick();
    await tick();
    assert.deepEqual(state.extended, []);
  } finally {
    restore();
  }
});

test("the preload waits for a room's EOSE and its catch-up", async () => {
  const { restore } = harness({ points: { w1: 5_000 } });
  let answer;
  const restorePage = setStoreCoverageDeps({
    requestPage: () => new Promise((resolve) => (answer = resolve)),
  });
  const subscription = bundle(["w1"]);
  try {
    let settled = false;
    const waiting = waitStoreLive(["w1"], 2_000).then(() => (settled = true));
    await new Promise((resolve) => setTimeout(resolve, 150));
    assert.equal(settled, false);
    noteStoreLiveEose(subscription, 1);
    await new Promise((resolve) => setTimeout(resolve, 150));
    // Live, but the catch-up has not landed yet.
    assert.equal(settled, false);
    answer([]);
    await waiting;
    assert.equal(settled, true);
  } finally {
    retireLiveSubscription(subscription);
    restorePage();
    restore();
  }
});

test("the preload's wait for a room that never goes live is capped", async () => {
  const started = Date.now();
  await waitStoreLive(["never-live"], 60);
  assert.ok(Date.now() - started >= 50);
});

test("an aborted preload stops waiting at once", async () => {
  const abort = new AbortController();
  const started = Date.now();
  setTimeout(() => abort.abort(), 20);
  await waitStoreLive(["never-live"], 2_000, abort.signal);
  assert.ok(Date.now() - started < 1_000);
});

test("a repair that ends without completing releases the preload", async () => {
  const { restore } = harness({ points: { w2: 5_000 } });
  const subscription = {
    ...bundle(["w2"]),
    reconnectReplay: { generation: 1, repairDone: false },
  };
  try {
    noteStoreLiveEose(subscription, 1);
    let settled = false;
    const waiting = waitStoreLive(["w2"], 2_000).then(() => (settled = true));
    await new Promise((resolve) => setTimeout(resolve, 50));
    assert.equal(settled, false);
    noteStoreRepairEnded(subscription);
    await waiting;
    assert.equal(settled, true);
  } finally {
    retireLiveSubscription(subscription);
    restore();
  }
});
