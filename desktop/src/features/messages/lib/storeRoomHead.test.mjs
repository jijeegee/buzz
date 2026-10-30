import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { afterEach, test } from "node:test";

import {
  CancelledError,
  QueryClient,
  QueryObserver,
} from "@tanstack/react-query";

import { channelMessagesKey } from "./messageQueryKeys.ts";

import {
  markStoreRoomHeadFetched,
  openChannelWindowEvents,
  resetStoreRoomHeadRooms,
  setStoreRoomHeadDeps,
} from "./storeRoomHead.ts";
import {
  isPaintedWindow,
  isStoredWindow,
  storeFollowUp,
} from "./storedWindows.ts";

// Shared rules with mobile (test-fixtures/message-store-paint-first.json).
const fixture = JSON.parse(
  readFileSync(
    new URL(
      "../../../../../test-fixtures/message-store-paint-first.json",
      import.meta.url,
    ),
    "utf8",
  ),
);

const stored = [{ id: "s1", kind: 9 }];
const paint = { window: stored, completeFrom: null };
const relay = [{ id: "r1", kind: 9 }];

afterEach(() => resetStoreRoomHeadRooms());

function deferred() {
  let resolve = () => {};
  let reject = () => {};
  const promise = new Promise((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

/** Store and relay answer when the test says so. */
function gatedHarness() {
  const calls = { paint: 0, relay: 0, refresh: [] };
  const store = deferred();
  const relayAnswer = deferred();
  const refreshed = deferred();
  const restore = setStoreRoomHeadDeps({
    roomPaint: () => {
      calls.paint += 1;
      return store.promise;
    },
    relayWindow: () => {
      calls.relay += 1;
      return relayAnswer.promise;
    },
    relayRefresh: async (queryClient, roomId) => {
      calls.refresh.push(roomId);
      // The refresh asks for the room's window as a real one would.
      try {
        await openChannelWindowEvents(queryClient, roomId);
      } finally {
        refreshed.resolve();
      }
    },
  });
  return { calls, restore, store, relayAnswer, refreshDone: refreshed.promise };
}

function harness({ head = stored } = {}) {
  const calls = { paint: 0, relay: 0, refresh: [] };
  let refreshed;
  const refreshDone = new Promise((resolve) => {
    refreshed = resolve;
  });
  const restore = setStoreRoomHeadDeps({
    roomPaint: async () => {
      calls.paint += 1;
      return head && { window: head, completeFrom: null };
    },
    // Slower than the store, as the network is.
    relayWindow: async () => {
      calls.relay += 1;
      await new Promise((resolve) => setTimeout(resolve, 5));
      return relay;
    },
    relayRefresh: async (queryClient, roomId) => {
      calls.refresh.push(roomId);
      await openChannelWindowEvents(queryClient, roomId);
      refreshed();
    },
  });
  return { calls, restore, refreshDone };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

for (const testCase of fixture.paint) {
  test(`paint: ${testCase.name}`, async () => {
    const { calls, restore, store, relayAnswer, refreshDone } = gatedHarness();
    try {
      const queryClient = new QueryClient();
      const opened = openChannelWindowEvents(queryClient, "room");
      const answerStore = () => {
        if (testCase.store === "rows") store.resolve(paint);
        else if (testCase.store === "empty") store.resolve(null);
        else store.reject(new Error("store read failed"));
      };
      const answerRelay = () => {
        if (testCase.relay === "ok") relayAnswer.resolve(relay);
        else relayAnswer.reject(new Error("relay failed"));
      };
      await settle();
      // The relay request goes out at once, before the store answers.
      assert.equal(calls.relay, 1);
      if (testCase.first === "store") {
        answerStore();
        await settle();
        answerRelay();
      } else {
        answerRelay();
        await settle();
        answerStore();
      }
      const expect = testCase.expect;
      if (expect.draws === "error") {
        await assert.rejects(opened);
      } else {
        const drawn = await opened;
        assert.deepEqual(drawn, expect.draws === "stored" ? stored : relay);
        assert.equal(isPaintedWindow(drawn), expect.draws === "stored");
      }
      if (expect.then === "relay") {
        await refreshDone;
        assert.deepEqual(calls.refresh, ["room"]);
      } else {
        await settle();
        assert.deepEqual(calls.refresh, []);
      }
      assert.equal(calls.relay, expect.relay_requests);
    } finally {
      restore();
    }
  });
}

test("a first open draws the stored window, and its refresh takes the request already out", async () => {
  const { calls, restore, refreshDone } = harness();
  try {
    const events = await openChannelWindowEvents(new QueryClient(), "room");
    assert.deepEqual(events, stored);
    assert.equal(calls.paint, 1);
    assert.equal(calls.relay, 1);
    await refreshDone;
    assert.deepEqual(calls.refresh, ["room"]);
    assert.equal(calls.relay, 1);
  } finally {
    restore();
  }
});

test("later fetches in this run ask the relay", async () => {
  const { calls, restore, refreshDone } = harness();
  try {
    const queryClient = new QueryClient();
    const first = await openChannelWindowEvents(queryClient, "room");
    await refreshDone;
    const later = await openChannelWindowEvents(queryClient, "room");
    assert.deepEqual(later, relay);
    assert.equal(calls.paint, 1);
    assert.equal(calls.relay, 2);
    // Only the relay's window is saved as the head cache.
    assert.equal(isStoredWindow(first), true);
    assert.equal(isStoredWindow(later), false);
  } finally {
    restore();
  }
});

test("nothing stored draws the relay's window with no refresh after it", async () => {
  const { calls, restore } = harness({ head: null });
  try {
    assert.deepEqual(
      await openChannelWindowEvents(new QueryClient(), "room"),
      relay,
    );
    await settle();
    assert.equal(calls.relay, 1);
    assert.equal(calls.refresh.length, 0);
  } finally {
    restore();
  }
});

test("through a real query: the stored rows show, then the relay's replace them", async () => {
  const queryClient = new QueryClient();
  const queryKey = channelMessagesKey("room");
  const seenAtRefresh = [];
  let relayCalls = 0;
  let refreshed;
  const refreshDone = new Promise((resolve) => {
    refreshed = resolve;
  });
  const restore = setStoreRoomHeadDeps({
    roomPaint: async () => paint,
    relayWindow: async () => {
      relayCalls += 1;
      await new Promise((resolve) => setTimeout(resolve, 5));
      return relay;
    },
    relayRefresh: async (client, roomId) => {
      seenAtRefresh.push(client.getQueryState(channelMessagesKey(roomId)));
      await client.refetchQueries({ queryKey: channelMessagesKey(roomId) });
      refreshed();
    },
  });
  const observer = new QueryObserver(queryClient, {
    queryKey,
    queryFn: () => openChannelWindowEvents(queryClient, "room"),
  });
  const unsubscribe = observer.subscribe(() => {});
  try {
    await refreshDone;
    assert.equal(seenAtRefresh.length, 1);
    assert.equal(seenAtRefresh[0].status, "success");
    assert.deepEqual(queryClient.getQueryData(queryKey), relay);
    assert.equal(relayCalls, 1);
  } finally {
    unsubscribe();
    restore();
    queryClient.clear();
  }
});

test("rooms are tracked per query cache, and a preloaded room skips the store", async () => {
  const { calls, restore } = harness();
  try {
    await openChannelWindowEvents(new QueryClient(), "room");
    // A new community's cache opens it from the store again.
    await openChannelWindowEvents(new QueryClient(), "room");
    assert.equal(calls.paint, 2);

    const preloaded = new QueryClient();
    markStoreRoomHeadFetched(preloaded, "room");
    assert.deepEqual(await openChannelWindowEvents(preloaded, "room"), relay);
    assert.equal(calls.paint, 2);
  } finally {
    restore();
  }
});

test("other refreshes join the relay request that follows a stored paint", async () => {
  let finishRefresh;
  const restore = setStoreRoomHeadDeps({
    roomPaint: async () => paint,
    relayWindow: () => new Promise(() => {}),
    relayRefresh: () =>
      new Promise((resolve) => {
        finishRefresh = resolve;
      }),
  });
  try {
    const queryClient = new QueryClient();
    await openChannelWindowEvents(queryClient, "room");
    const followUp = storeFollowUp(queryClient, "room");
    assert.ok(followUp, "the room's refreshes have one request to join");
    await settle();
    finishRefresh();
    await followUp;
    await Promise.resolve();
    assert.equal(storeFollowUp(queryClient, "room"), undefined);
  } finally {
    restore();
  }
});

test("a first fetch overtaken by a refresh is cancelled; the refresh takes its request", async () => {
  const store = deferred();
  const calls = { relay: 0, refresh: 0 };
  const restore = setStoreRoomHeadDeps({
    roomPaint: () => store.promise,
    relayWindow: () => {
      calls.relay += 1;
      return new Promise((resolve) => setTimeout(() => resolve(relay), 5));
    },
    relayRefresh: async () => {
      calls.refresh += 1;
    },
  });
  try {
    const queryClient = new QueryClient();
    const firstFetch = openChannelWindowEvents(queryClient, "room");
    await settle();
    // A refresh cancels the first fetch and takes its relay request.
    assert.deepEqual(await openChannelWindowEvents(queryClient, "room"), relay);
    store.resolve(paint);
    await assert.rejects(firstFetch, CancelledError);
    await settle();
    assert.equal(calls.relay, 1);
    assert.equal(calls.refresh, 0);
    assert.equal(storeFollowUp(queryClient, "room"), undefined);
  } finally {
    restore();
  }
});

test("the relay's window is compared with the stored one it replaces", async () => {
  const lines = [];
  const info = console.info;
  const warn = console.warn;
  console.info = (line) => lines.push(line);
  console.warn = (line) => lines.push(line);
  const { restore, refreshDone } = harness();
  try {
    const queryClient = new QueryClient();
    await openChannelWindowEvents(queryClient, "room");
    assert.equal(lines.length, 0);
    await refreshDone;
    assert.equal(lines.length, 1);
    assert.ok(lines[0].startsWith("[message-store] room room: store "));
    // Only the first relay answer after the stored paint is compared.
    await openChannelWindowEvents(queryClient, "room");
    assert.equal(lines.length, 1);
  } finally {
    console.info = info;
    console.warn = warn;
    restore();
  }
});

test("a refetch that took the relay request leaves the follow-up nothing to ask", async () => {
  const calls = { relay: 0, refresh: 0 };
  const restore = setStoreRoomHeadDeps({
    roomPaint: async () => paint,
    relayWindow: async () => {
      calls.relay += 1;
      await new Promise((resolve) => setTimeout(resolve, 5));
      return relay;
    },
    relayRefresh: async () => {
      calls.refresh += 1;
    },
  });
  const queryClient = new QueryClient();
  // The room's query is still settling when the stored rows are drawn.
  const settle = deferred();
  void queryClient.fetchQuery({
    queryKey: channelMessagesKey("room"),
    queryFn: () => settle.promise,
  });
  try {
    assert.deepEqual(
      await openChannelWindowEvents(queryClient, "room"),
      stored,
    );
    const followUp = storeFollowUp(queryClient, "room");
    // Another refresh (after subscribing) refetches meanwhile and takes it.
    assert.deepEqual(await openChannelWindowEvents(queryClient, "room"), relay);
    settle.resolve(stored);
    await followUp;
    assert.equal(calls.refresh, 0);
    assert.equal(calls.relay, 1);
  } finally {
    restore();
    queryClient.clear();
  }
});

test("a painted room keeps how far back it is complete until the relay answers", async () => {
  const { paintCoverage } = await import("./paintCoverage.ts");
  const relayAnswer = deferred();
  const restore = setStoreRoomHeadDeps({
    roomPaint: async () => ({ window: stored, completeFrom: 180 }),
    relayWindow: () => relayAnswer.promise,
    relayRefresh: async (queryClient, roomId) => {
      await openChannelWindowEvents(queryClient, roomId);
    },
  });
  try {
    const queryClient = new QueryClient();
    await openChannelWindowEvents(queryClient, "room");
    assert.equal(paintCoverage("room"), 180);
    relayAnswer.resolve(relay);
    await storeFollowUp(queryClient, "room");
    assert.equal(paintCoverage("room"), undefined);
  } finally {
    restore();
  }
});
