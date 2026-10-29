import assert from "node:assert/strict";
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
import { isStoredWindow, storeFollowUp } from "./storedWindows.ts";

const stored = [{ id: "s1", kind: 9 }];
const relay = [{ id: "r1", kind: 9 }];

afterEach(() => resetStoreRoomHeadRooms());

function harness({ liveAt = 100, head = stored } = {}) {
  const calls = { roomHead: [], relay: 0, refresh: [] };
  let refreshed;
  const refreshDone = new Promise((resolve) => {
    refreshed = resolve;
  });
  const restore = setStoreRoomHeadDeps({
    liveAt: () => liveAt,
    roomHead: async (roomId, at) => {
      calls.roomHead.push([roomId, at]);
      return head;
    },
    relayWindow: async () => {
      calls.relay += 1;
      return relay;
    },
    relayRefresh: async (_queryClient, roomId) => {
      calls.refresh.push(roomId);
      refreshed();
    },
  });
  return { calls, restore, refreshDone };
}

test("a first open takes the certain stored window, then asks the relay", async () => {
  const { calls, restore, refreshDone } = harness();
  try {
    const events = await openChannelWindowEvents(new QueryClient(), "room");
    assert.deepEqual(events, stored);
    assert.deepEqual(calls.roomHead, [["room", 100]]);
    assert.equal(calls.relay, 0);
    await refreshDone;
    assert.deepEqual(calls.refresh, ["room"]);
  } finally {
    restore();
  }
});

test("later fetches in this run ask the relay, even with no data yet", async () => {
  const { calls, restore } = harness();
  try {
    const queryClient = new QueryClient();
    const first = await openChannelWindowEvents(queryClient, "room");
    const second = await openChannelWindowEvents(queryClient, "room");
    assert.deepEqual(second, relay);
    assert.equal(calls.roomHead.length, 1);
    // Only the relay's window is saved as the head cache.
    assert.equal(isStoredWindow(first), true);
    assert.equal(isStoredWindow(second), false);
  } finally {
    restore();
  }
});

test("no live REQ or no certain window asks the relay", async () => {
  for (const options of [{ liveAt: null }, { head: null }]) {
    resetStoreRoomHeadRooms();
    const { calls, restore } = harness(options);
    try {
      assert.deepEqual(
        await openChannelWindowEvents(new QueryClient(), "room"),
        relay,
      );
      assert.equal(calls.relay, 1);
      assert.equal(calls.refresh.length, 0);
    } finally {
      restore();
    }
  }
});

test("through a real query: the relay is asked once, after the stored fetch settles", async () => {
  const queryClient = new QueryClient();
  const queryKey = channelMessagesKey("room");
  const seenAtRefresh = [];
  let refreshed;
  const refreshDone = new Promise((resolve) => {
    refreshed = resolve;
  });
  const restore = setStoreRoomHeadDeps({
    liveAt: () => 100,
    roomHead: async () => stored,
    relayWindow: async () => relay,
    relayRefresh: async (client, roomId) => {
      seenAtRefresh.push(client.getQueryState(channelMessagesKey(roomId)));
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
    assert.equal(seenAtRefresh[0].fetchStatus, "idle");
    assert.deepEqual(queryClient.getQueryData(queryKey), stored);
    // A refetch in this run goes to the relay and asks nothing more.
    await observer.refetch();
    assert.deepEqual(queryClient.getQueryData(queryKey), relay);
    assert.equal(seenAtRefresh.length, 1);
  } finally {
    unsubscribe();
    restore();
    queryClient.clear();
  }
});

test("rooms are tracked per query cache, and a preloaded room skips the store", async () => {
  const { calls, restore } = harness();
  try {
    const first = new QueryClient();
    await openChannelWindowEvents(first, "room");
    // A new community's cache opens it from the store again.
    const second = new QueryClient();
    await openChannelWindowEvents(second, "room");
    assert.equal(calls.roomHead.length, 2);

    const preloaded = new QueryClient();
    markStoreRoomHeadFetched(preloaded, "room");
    assert.deepEqual(await openChannelWindowEvents(preloaded, "room"), relay);
    assert.equal(calls.roomHead.length, 2);
  } finally {
    restore();
  }
});

test("other refreshes join the relay request that follows a store open", async () => {
  let finishRefresh;
  const restore = setStoreRoomHeadDeps({
    liveAt: () => 100,
    roomHead: async () => stored,
    relayWindow: async () => relay,
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
    await new Promise((resolve) => setTimeout(resolve, 0));
    finishRefresh();
    await followUp;
    await Promise.resolve();
    assert.equal(storeFollowUp(queryClient, "room"), undefined);
  } finally {
    restore();
  }
});

test("a store read overtaken by a relay request asks nothing more", async () => {
  let answerStore;
  const calls = { relay: 0, refresh: 0 };
  const restore = setStoreRoomHeadDeps({
    liveAt: () => 100,
    roomHead: () =>
      new Promise((resolve) => {
        answerStore = resolve;
      }),
    relayWindow: async () => {
      calls.relay += 1;
      return relay;
    },
    relayRefresh: async () => {
      calls.refresh += 1;
    },
  });
  try {
    const queryClient = new QueryClient();
    const storeFetch = openChannelWindowEvents(queryClient, "room");
    // A refresh cancels the store fetch and asks the relay meanwhile.
    assert.deepEqual(await openChannelWindowEvents(queryClient, "room"), relay);
    answerStore(stored);
    // The cancelled fetch's answer is discarded; it asks nothing itself.
    await assert.rejects(storeFetch, CancelledError);
    await new Promise((resolve) => setTimeout(resolve, 0));
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
  const { restore } = harness();
  try {
    const queryClient = new QueryClient();
    await openChannelWindowEvents(queryClient, "room");
    assert.equal(lines.length, 0);
    await openChannelWindowEvents(queryClient, "room");
    assert.equal(lines.length, 1);
    assert.ok(lines[0].startsWith("[message-store] room room: store "));
    // Only the first relay answer after the store open is compared.
    await openChannelWindowEvents(queryClient, "room");
    assert.equal(lines.length, 1);
  } finally {
    console.info = info;
    console.warn = warn;
    restore();
  }
});
