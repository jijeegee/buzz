// The refresh after an open room's live subscription is set up: skipped only
// when the open's own window request went out with its listener attached and
// the room's chunk past EOSE, both unchanged since. Real hooks, RelayClient
// room bundle and store coverage; mocked relay subscription and Tauri IPC.
import assert from "node:assert/strict";
import { after, afterEach, before, mock, test } from "node:test";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { JSDOM } from "jsdom";
import React from "react";

import { useChannelMessagesQuery, useChannelSubscription } from "./hooks.ts";
import { preloadChannelsMessages } from "./channelPreload.ts";
import { channelMessagesKey } from "./lib/messageQueryKeys.ts";
import { setStoreScope } from "./lib/paintCoverage.ts";
import { relayClient } from "../../shared/api/relayClient.ts";
import { retireLiveSubscription } from "../../shared/api/relayClosedRecovery.ts";
import { noteStoreLiveEose } from "../../shared/api/relayStoreCoverage.ts";

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://localhost",
});
before(() => {
  Object.assign(globalThis, {
    document: dom.window.document,
    HTMLElement: dom.window.HTMLElement,
    IS_REACT_ACT_ENVIRONMENT: true,
    window: dom.window,
  });
});
const clients = [];
const chunkSubscriptions = [];
afterEach(async () => {
  const { cleanup } = await import("@testing-library/react");
  cleanup();
  for (const client of clients.splice(0)) client.clear();
  for (const subscription of chunkSubscriptions.splice(0)) {
    retireLiveSubscription(subscription);
  }
  mock.restoreAll();
  relayClient.disconnect();
});
after(() => dom.window.close());

const room = "room-window-token";
const channel = {
  id: room,
  name: "general",
  channelType: "stream",
  visibility: "open",
  description: "",
  topic: null,
  purpose: null,
  memberCount: 1,
  memberPubkeys: [],
  lastMessageAt: null,
  archivedAt: null,
  participants: [],
  participantPubkeys: [],
  isMember: true,
  ttlSeconds: null,
  ttlDeadline: null,
};
const row = {
  id: "1".repeat(64),
  pubkey: "b".repeat(64),
  created_at: 100,
  kind: 9,
  tags: [["h", room]],
  content: "hello",
  sig: "",
};
const bounds = {
  id: "9".repeat(64),
  pubkey: "e".repeat(64),
  created_at: 101,
  kind: 39006,
  tags: [
    ["h", room],
    ["d", `${room}:head`],
  ],
  content: JSON.stringify({ has_more: false, next_cursor: null }),
  sig: "",
};

function deferred() {
  let resolve = () => {};
  const promise = new Promise((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

/**
 * The room's chunk is opened (its REQ not yet at EOSE). `eose()` marks it
 * live in the store's coverage, as the relay session does at EOSE. Room
 * listens attach at once but resolve only on `releaseListen()`, so the test
 * decides whether the refresh runs before or after the window request.
 */
async function setUpRoom() {
  window.localStorage.clear();
  const calls = { window: 0, windows: 0, paints: [] };
  const windowSent = deferred();
  const storeHead = deferred();
  storeHead.resolve();
  const harnessState = { storeHead: storeHead.promise };
  window.__TAURI_INTERNALS__ = {
    invoke: async (command, args) => {
      if (command === "message_store_room_paint") {
        calls.paints.push(args);
        await harnessState.storeHead;
        return null;
      }
      if (command === "get_channel_window") {
        calls.window += 1;
        windowSent.resolve();
        return [row, bounds];
      }
      if (command === "get_channel_windows") {
        calls.windows += 1;
        return [{ channel_id: room, events: [row, bounds] }];
      }
      return null;
    },
  };
  mock.method(relayClient, "subscribeToReconnects", () => () => {});
  const opened = deferred();
  mock.method(relayClient, "subscribeLive", async (filter, onEvent) => {
    const subscription = { mode: "live", filter, onEvent };
    chunkSubscriptions.push(subscription);
    opened.resolve(subscription);
    return async () => retireLiveSubscription(subscription);
  });
  const listen = deferred();
  mock.method(
    relayClient,
    "subscribeToChannelLive",
    async (roomId, onEvent) => {
      const stop = relayClient.liveRooms.listenRoom(roomId, onEvent);
      await listen.promise;
      return stop;
    },
  );
  relayClient.liveRooms.setMemberRooms([room]);
  const chunk = await opened.promise;
  let generation = 0;
  return {
    calls,
    chunk,
    windowSent: windowSent.promise,
    eose: () => noteStoreLiveEose(chunk, ++generation),
    releaseListen: () => listen.resolve(),
    /** Hold the open's device-store paint read; returns its release. */
    holdStoreHead: () => {
      const held = deferred();
      harnessState.storeHead = held.promise;
      return () => held.resolve();
    },
  };
}

/** ChannelScreen's order: the query hook first. */
function useOpenQueryFirst() {
  const query = useChannelMessagesQuery(channel);
  useChannelSubscription(channel);
  return query;
}

function useOpenSubscriptionFirst() {
  useChannelSubscription(channel);
  return useChannelMessagesQuery(channel);
}

async function openRoom(client, { queryFirst = false } = {}) {
  const { act, renderHook, waitFor } = await import("@testing-library/react");
  const view = renderHook(
    queryFirst ? useOpenQueryFirst : useOpenSubscriptionFirst,
    {
      wrapper: ({ children }) =>
        React.createElement(QueryClientProvider, { client }, children),
    },
  );
  const settle = async () => {
    await waitFor(() =>
      assert.ok(view.result.current.data?.some((event) => event.id === row.id)),
    );
    // Let the refresh after subscribing finish (or ask the relay).
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
  };
  return { settle, waitFor };
}

function newClient() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  // The paint reads only in the cache's own account and server.
  setStoreScope(client, { pubkey: "a".repeat(64), relayUrl: "wss://relay" });
  clients.push(client);
  return client;
}

/**
 * A room visited earlier whose rows went stale: opening refetches them while
 * they show. (A first open has no rows, and TanStack joins the refresh onto
 * that fetch instead of sending another.)
 */
function revisitClient() {
  const client = newClient();
  client.setQueryData(channelMessagesKey(room), [row], {
    updatedAt: Date.now() - 10 * 60_000,
  });
  return client;
}

test("an open whose request went out with the chunk live asks the relay once", async () => {
  const harness = await setUpRoom();
  harness.eose();
  const { settle } = await openRoom(revisitClient());
  await harness.windowSent;
  harness.releaseListen();
  await settle();
  assert.equal(harness.calls.window, 1);
});

test("in ChannelScreen's hook order, the open's request goes out with the listener, without waiting for the store", async () => {
  const harness = await setUpRoom();
  harness.eose();
  const releaseStoreHead = harness.holdStoreHead();
  const client = revisitClient();
  let invalidations = 0;
  const invalidate = client.invalidateQueries.bind(client);
  mock.method(client, "invalidateQueries", (...args) => {
    invalidations += 1;
    return invalidate(...args);
  });
  const { settle } = await openRoom(client, { queryFirst: true });
  // The device store read is still out: the window request does not wait
  // for it, and the listener attached in the same commit goes with it.
  await harness.windowSent;
  harness.releaseListen();
  await settle();
  releaseStoreHead();
  assert.equal(harness.calls.paints.length, 1);
  assert.equal(harness.calls.paints[0].account, "a".repeat(64));
  assert.equal(harness.calls.paints[0].relayUrl, "wss://relay");
  assert.equal(harness.calls.window, 1);
  assert.equal(invalidations, 0);
});

test("a chunk not yet live when the request went out still refreshes", async () => {
  const harness = await setUpRoom();
  const { settle, waitFor } = await openRoom(revisitClient());
  await harness.windowSent;
  harness.eose();
  harness.releaseListen();
  await settle();
  await waitFor(() => assert.equal(harness.calls.window, 2));
});

test("a chunk that stopped receiving after the request refreshes", async () => {
  const harness = await setUpRoom();
  harness.eose();
  const { settle, waitFor } = await openRoom(revisitClient());
  await harness.windowSent;
  // Socket lost and the REQ live again: events in between may be missing.
  retireLiveSubscription(harness.chunk);
  harness.eose();
  harness.releaseListen();
  await settle();
  await waitFor(() => assert.equal(harness.calls.window, 2));
});

test("a window preloaded without a room listener does not count", async () => {
  const harness = await setUpRoom();
  harness.eose();
  const client = newClient();
  assert.deepEqual(await preloadChannelsMessages(client, [room], 50), [room]);
  const { settle, waitFor } = await openRoom(client);
  harness.releaseListen();
  await settle();
  await waitFor(() => assert.equal(harness.calls.window, 1));
  assert.equal(harness.calls.windows, 1);
});
