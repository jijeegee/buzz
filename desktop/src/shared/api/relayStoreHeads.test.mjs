import assert from "node:assert/strict";
import test from "node:test";

import { CHANNEL_EVENT_KINDS } from "@/shared/constants/kinds";
import { retireLiveSubscription } from "./relayClosedRecovery.ts";
import {
  noteStoreLiveEose,
  setStoreCoverageDeps,
} from "./relayStoreCoverage.ts";
import {
  recordsStoreHead,
  requestWithStoreHeads,
  roomWindowAnswer,
  setStoreHeadDeps,
} from "./relayStoreHeads.ts";

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

function harness({ scope = "stem:1:7" } = {}) {
  const state = { now: 10_000, sent: [], recorded: [] };
  const restoreCoverage = setStoreCoverageDeps({
    nowSecs: () => state.now,
    scope: async () => "s1",
    extend: async () => {},
    resumePoints: async () => ({}),
    requestPage: async () => [],
  });
  const restoreHeads = setStoreHeadDeps({
    sent: async (roomIds) => {
      state.sent.push(roomIds);
      return scope;
    },
    record: async (capture, heads) => {
      state.recorded.push([capture, heads]);
    },
  });
  return {
    state,
    restore: () => {
      restoreHeads();
      restoreCoverage();
    },
  };
}

const bundle = (rooms) => ({
  mode: "live",
  filter: { kinds: [...CHANNEL_EVENT_KINDS], "#h": rooms, limit: 0 },
  onEvent: () => {},
});

const bounds = (roomId, hasMore, nextCursor = null) => ({
  id: `bounds-${roomId}`,
  kind: 39006,
  created_at: 10_001,
  tags: [["d", `${roomId}:head`]],
  content: JSON.stringify({ has_more: hasMore, next_cursor: nextCursor }),
});

const row = (id, createdAt) => ({
  id,
  kind: 9,
  created_at: createdAt,
  tags: [],
});

test("a newest window under a live REQ records its head", async () => {
  const { state, restore } = harness();
  try {
    noteStoreLiveEose(bundle(["r1"]), 1);
    await tick();
    state.now = 10_500;
    const events = [row("a", 9_000), bounds("r1", false)];
    const answer = await requestWithStoreHeads(
      ["r1", "r2"],
      async () => events,
      (events) => [roomWindowAnswer("r1", events)],
    );
    await tick();
    assert.equal(answer, events);
    // Only the live room is marked.
    assert.deepEqual(state.sent, [["r1"]]);
    assert.deepEqual(state.recorded, [
      [
        "stem:1:7",
        [
          {
            roomId: "r1",
            rootId: null,
            liveAt: 10_000,
            nextCursor: null,
            ids: ["a"],
          },
        ],
      ],
    ]);
  } finally {
    restore();
  }
});

test("a REQ that stopped during the request records no head", async () => {
  const { state, restore } = harness();
  try {
    const subscription = bundle(["r1"]);
    noteStoreLiveEose(subscription, 1);
    await tick();
    await requestWithStoreHeads(
      ["r1"],
      async () => {
        retireLiveSubscription(subscription);
        return [row("a", 9_000), bounds("r1", false)];
      },
      (events) => [roomWindowAnswer("r1", events)],
    );
    await tick();
    // The mark is released with nothing recorded.
    assert.deepEqual(state.recorded, [["stem:1:7", []]]);
  } finally {
    restore();
  }
});

test("a room no live REQ carries is never marked", async () => {
  const { state, restore } = harness();
  try {
    const answer = await requestWithStoreHeads(
      ["quiet"],
      async () => "answer",
      () => [],
    );
    assert.equal(answer, "answer");
    assert.deepEqual(state.sent, []);
    assert.deepEqual(state.recorded, []);
  } finally {
    restore();
  }
});

test("a failed request releases its mark", async () => {
  const { state, restore } = harness();
  try {
    noteStoreLiveEose(bundle(["r1"]), 1);
    await tick();
    await assert.rejects(
      requestWithStoreHeads(
        ["r1"],
        async () => {
          throw new Error("offline");
        },
        () => [],
      ),
    );
    await tick();
    assert.deepEqual(state.recorded, [["stem:1:7", []]]);
  } finally {
    restore();
  }
});

test("window bounds give the head's oldest row", () => {
  const cursor = { created_at: 8_000, id: "z" };
  const answer = roomWindowAnswer("r1", [
    row("a", 9_000),
    bounds("r1", true, cursor),
  ]);
  assert.equal(answer.hasMore, true);
  assert.deepEqual(answer.nextCursor, { createdAt: 8_000, eventId: "z" });
  assert.deepEqual(
    answer.events.map((event) => event.id),
    ["a"],
  );
  // Bounds of an older page, or disagreeing, are no head.
  const older = { ...bounds("r1", false), tags: [["d", "r1:8000:z"]] };
  assert.equal(roomWindowAnswer("r1", [older]), null);
  assert.equal(roomWindowAnswer("r1", [bounds("r1", true)]), null);
});

test("only an opted-in request at the open size records", () => {
  assert.equal(recordsStoreHead({ storeHead: true }, 50), true);
  assert.equal(recordsStoreHead({ storeHead: true }, 25), false);
  assert.equal(recordsStoreHead({ storeHead: true }, 20), false);
  assert.equal(recordsStoreHead({}, 50), false);
});
