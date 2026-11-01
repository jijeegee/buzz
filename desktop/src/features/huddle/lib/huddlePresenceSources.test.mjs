import assert from "node:assert/strict";
import test from "node:test";

import {
  HUDDLE_COVERAGE_WAITS,
  startHuddlePresenceRuntime,
} from "./huddlePresenceRuntime.ts";
import {
  HUDDLE_LIFECYCLE_KINDS,
  huddleWindowPresenceSource,
  roomBundlePresenceSource,
} from "./huddlePresenceSources.ts";

const RELAY = "c".repeat(64);

function lifecycle(id, kind, room) {
  return {
    id,
    kind,
    pubkey: RELAY,
    content: "{}",
    tags: [["h", room]],
    created_at: 1_000,
    sig: "",
  };
}

async function settle() {
  for (let i = 0; i < 10; i++) {
    await new Promise((resolve) => setImmediate(resolve));
  }
}

test("the main window hears its rooms' lifecycle events on the bundle", async () => {
  const listeners = new Set();
  const covered = [];
  const source = roomBundlePresenceSource({
    channelIds: ["general", "design"],
    listenAllRooms: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    whenRoomsCovered: async (roomIds) => {
      covered.push(roomIds);
      return true;
    },
  });
  const heard = [];
  const stop = await source.listenLive((event) => heard.push(event.id));
  const emit = (event) => {
    for (const listener of listeners) listener(event);
  };
  emit(lifecycle("join", 48101, "general"));
  emit(lifecycle("message", 9, "general"));
  emit(lifecycle("liveness", 48104, "general"));
  emit(lifecycle("elsewhere", 48101, "random"));
  assert.deepEqual(heard, ["join"]);
  assert.equal(await source.whenLive(), true);
  assert.deepEqual(covered, [["general", "design"]]);
  await stop();
  assert.equal(listeners.size, 0);
});

test("the huddle window keeps a limit:0 lifecycle REQ per 128 rooms, no since", async () => {
  const filters = [];
  const disposed = [];
  const channelIds = Array.from(
    { length: 130 },
    (_, i) => `room-${String(i).padStart(3, "0")}`,
  );
  const source = huddleWindowPresenceSource({
    channelIds,
    subscribeLive: async (filter) => {
      filters.push(filter);
      return () => disposed.push(filter);
    },
  });
  const stop = await source.listenLive(() => {});
  assert.deepEqual(
    filters.map((filter) => filter["#h"].length),
    [128, 2],
  );
  for (const filter of filters) {
    assert.deepEqual(filter.kinds, [...HUDDLE_LIFECYCLE_KINDS]);
    assert.equal(filter.limit, 0);
    assert.equal("since" in filter, false);
  }
  await stop();
  assert.equal(disposed.length, 2);
  // Re-sent byte-identical.
  const first = JSON.stringify(filters);
  filters.length = 0;
  await source.listenLive(() => {});
  assert.equal(JSON.stringify(filters), first);
  assert.equal(await source.whenLive(), true);
});

/** The runtime over a scripted source; records the order of calls. */
function runtimeWith(liveAnswers) {
  const calls = [];
  let liveCalls = 0;
  const dispose = startHuddlePresenceRuntime({
    relaySelfPubkey: RELAY,
    channelIds: ["general"],
    listenLive: async () => {
      calls.push("listen");
      return () => {};
    },
    whenLive: async () => {
      const answer = liveAnswers[Math.min(liveCalls, liveAnswers.length - 1)];
      liveCalls += 1;
      calls.push(`live:${answer}`);
      return answer;
    },
    fetchEvents: async (filter) => {
      if (filter.kinds?.includes(48100)) calls.push("history");
      return [];
    },
    subscribeToReconnects: () => () => {},
    onPresence: () => {},
    setLivenessTimer: () => 1,
    clearLivenessTimer: () => {},
    nowSeconds: () => 1_000,
  });
  return { calls, dispose };
}

test("the runtime attaches before it reads history", async () => {
  const { calls, dispose } = runtimeWith([true]);
  await settle();
  assert.deepEqual(calls, ["listen", "live:true", "history"]);
  dispose();
});

test("late coverage: reconcile again once the rooms are live", async () => {
  const { calls, dispose } = runtimeWith([false, false, true]);
  await settle();
  assert.deepEqual(calls, [
    "listen",
    "live:false",
    "history",
    "live:false",
    "live:true",
    "history",
  ]);
  dispose();
});

test("coverage that never comes is waited for a bounded number of times", async () => {
  // Also the zero-member-rooms bundle: its bound is always hit.
  const { calls, dispose } = runtimeWith([false]);
  await settle();
  assert.equal(calls.filter((call) => call === "history").length, 1);
  assert.equal(
    calls.filter((call) => call === "live:false").length,
    1 + HUDDLE_COVERAGE_WAITS,
  );
  dispose();
});
