import assert from "node:assert/strict";
import test from "node:test";

import { CHANNEL_EVENT_KINDS } from "@/shared/constants/kinds";
import { retireLiveSubscription } from "./relayClosedRecovery.ts";
import { RECONNECT_REPLAY_CHANNEL_LOOKBACK_SECS } from "./relayReconnectReplay.ts";
import {
  catchUpFromStore,
  noteStoreLiveEose,
  noteStoreLiveEvent,
  setStoreCoverageDeps,
  storeLiveAt,
  STORE_CATCH_UP_MAX_PAGES,
  STORE_CATCH_UP_PAGE_LIMIT,
  STORE_RESUME_LOOKBACK_SECS,
} from "./relayStoreCoverage.ts";

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

function harness({ now = 10_000, points = {}, pages = [] } = {}) {
  const state = { now, extended: [], requests: [] };
  const restore = setStoreCoverageDeps({
    nowSecs: () => state.now,
    extend: async (roomIds, from, to) => {
      state.extended.push([roomIds, from, to]);
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
    assert.equal(state.requests[0].until, 10_000);
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

test("events extend coverage at most once a minute, on the same connection", () => {
  const { state, restore } = harness();
  try {
    const subscription = bundle();
    noteStoreLiveEose(subscription, 1);
    state.now += 30;
    noteStoreLiveEvent(subscription, 1);
    state.now += 40;
    noteStoreLiveEvent(subscription, 2);
    noteStoreLiveEvent(subscription, 1);
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
