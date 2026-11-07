// R1: a newest room or thread window held in the store paints at once in
// every mode; the window check runs behind the paint and never holds it.
import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

import { QueryClient } from "@tanstack/react-query";

import { setStoreScope } from "./paintCoverage.ts";
import {
  openChannelWindowEvents,
  resetStoreRoomHeadRooms,
  setStoreRoomHeadDeps,
} from "./storeRoomHead.ts";
import { setStoreThreadHeadDeps } from "./storeThreadHead.ts";
import { loadNewestThreadReplies } from "./threadWindowCache.ts";
import { resetWindowCheckGuards, setWindowCheckDeps } from "./windowCheck.ts";
import {
  noteServerWindowCheckMode,
  resetWindowCheckModes,
} from "./windowCheckMode.ts";

const RELAY = "wss://relay.test";
const never = () => new Promise(() => {});

function client() {
  const queryClient = new QueryClient();
  setStoreScope(queryClient, { pubkey: "me", relayUrl: RELAY });
  return queryClient;
}

/** The store holds the window; the relay (mark, check, page) never answers. */
function stalledRelay() {
  const asked = { stored: 0, check: 0 };
  const restore = setWindowCheckDeps({
    stored: async () => {
      asked.stored += 1;
      return {
        events: [],
        count: 1,
        oldest: { createdAt: 1, eventId: "a" },
        hasMore: false,
        fullyStored: true,
      };
    },
    check: () => {
      asked.check += 1;
      return never();
    },
    mark: never,
  });
  return { asked, restore };
}

let restores = [];
afterEach(() => {
  for (const restore of restores) restore();
  restores = [];
  resetStoreRoomHeadRooms();
  resetWindowCheckModes();
  resetWindowCheckGuards();
});

for (const mode of ["on", "shadow", "unknown"]) {
  test(`a stored room window paints before the check (${mode})`, async () => {
    if (mode !== "unknown") noteServerWindowCheckMode(RELAY, mode, Date.now());
    const relay = stalledRelay();
    const painted = [{ id: "s1", kind: 9 }];
    restores.push(
      relay.restore,
      setStoreRoomHeadDeps({
        roomPaint: async () => ({ window: painted, completeFrom: null }),
        relayRefresh: async () => {},
      }),
    );
    const drawn = await openChannelWindowEvents(client(), "room");
    assert.deepEqual(drawn, painted);
    // The check's path started; it is still out.
    assert.equal(relay.asked.check + relay.asked.stored >= 1, true);
  });

  test(`a stored thread window paints before the check (${mode})`, async () => {
    if (mode !== "unknown") noteServerWindowCheckMode(RELAY, mode, Date.now());
    const relay = stalledRelay();
    const reply = {
      id: "r1",
      kind: 9,
      created_at: 5,
      tags: [["e", "root", "", "reply"]],
    };
    restores.push(
      relay.restore,
      setStoreThreadHeadDeps({
        threadPaint: async () => ({
          window: { events: [reply], has_more: false, next_cursor: null },
          complete_from: null,
        }),
        relayRefresh: async () => {},
      }),
    );
    const events = await loadNewestThreadReplies(client(), "room", "root");
    assert.deepEqual(
      events.map((event) => event.id),
      ["r1"],
    );
    assert.equal(relay.asked.check + relay.asked.stored >= 1, true);
  });
}
