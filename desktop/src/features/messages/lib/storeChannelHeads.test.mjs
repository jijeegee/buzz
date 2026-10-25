import assert from "node:assert/strict";
import { after, afterEach, before, mock, test } from "node:test";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { JSDOM } from "jsdom";
import React from "react";
import {
  applyStoreHead,
  consumeHydratedChannel,
  hasPersistedHydratedChannel,
  hydrateChannelHeads,
  isProvenStoreHead,
  storeWindowPage,
} from "./storeChannelHeads.ts";
import { channelMessagesKey, channelWindowKey } from "./messageQueryKeys.ts";
import { refreshChannelWindowMessages } from "./projectChannelWindow.ts";
import { pageOlderMessagesUntilRowFloor } from "./pageOlderMessages.ts";
import { useChannelMessagesQuery, useChannelSubscription } from "../hooks.ts";
import { relayClient } from "../../../shared/api/relayClient.ts";

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://localhost",
});
const channelId = "channel-a";
const iso = (seconds) => new Date(seconds * 1000).toISOString();
const channel = {
  id: channelId,
  name: "general",
  channelType: "stream",
  visibility: "open",
  description: "",
  topic: null,
  purpose: null,
  memberCount: 1,
  memberPubkeys: [],
  lastMessageAt: iso(10),
  archivedAt: null,
  participants: [],
  participantPubkeys: [],
  isMember: true,
  ttlSeconds: null,
  ttlDeadline: null,
};
const scope = { pubkey: "f".repeat(64), relayUrl: "wss://relay" };
const root = {
  id: "a".repeat(64),
  pubkey: "b".repeat(64),
  created_at: 10,
  kind: 40002,
  tags: [["h", channelId]],
  content: "stored",
  sig: "",
};
const replacement = {
  ...root,
  id: "c".repeat(64),
  created_at: 11,
  content: "relay",
};
function bounds(suffix = "head", next = null) {
  return {
    id: "d".repeat(64),
    pubkey: "e".repeat(64),
    created_at: 12,
    kind: 39006,
    tags: [
      ["h", channelId],
      ["d", `${channelId}:${suffix}`],
    ],
    content: JSON.stringify({ has_more: next !== null, next_cursor: next }),
    sig: "",
  };
}
const storedPage = (rows = [root], extra = {}) => ({
  startCursor: null,
  rows,
  aux: [],
  summaries: [],
  nextCursor: null,
  hasMore: false,
  gap: false,
  ...extra,
});
const coverageFor = ({ hiAt = 10, liveCovered = false } = {}) => ({
  channelId,
  ranges: [{ class: "top", loAt: 0, loId: "", loIsStart: true, hiAt }],
  liveCovered,
  provenThrough: null,
  newestLocalAt: hiAt,
  gapBelow: false,
});

let calls = [];
let store;
function install({
  heads = [storedPage()],
  coverage = {},
  window: storeWindow = storedPage(),
  loadDelayMs = 0,
} = {}) {
  calls = [];
  store = { heads, coverage, window: storeWindow };
  window.localStorage.clear();
  window.__TAURI_INTERNALS__ = {
    invoke: async (command, args) => {
      calls.push({ command, args });
      switch (command) {
        case "message_store_load_heads":
          if (loadDelayMs > 0) {
            await new Promise((resolve) => setTimeout(resolve, loadDelayMs));
          }
          return store.heads.map((page) => ({ channelId, page }));
        case "message_store_coverage":
          return [coverageFor(store.coverage)];
        case "message_store_read_window":
          return typeof store.window === "function"
            ? store.window(args)
            : store.window;
        case "get_channel_window":
          // Rust stores every server window with what it proves.
          store.window = storedPage([replacement]);
          store.coverage = { ...store.coverage, hiAt: 12 };
          return [replacement, bounds()];
        default:
          return null;
      }
    },
  };
}
const serverCalls = () =>
  calls.filter((call) => call.command === "get_channel_window").length;

before(() => {
  Object.assign(globalThis, {
    document: dom.window.document,
    HTMLElement: dom.window.HTMLElement,
    IS_REACT_ACT_ENVIRONMENT: true,
    window: dom.window,
  });
});

afterEach(async () => {
  const { cleanup } = await import("@testing-library/react");
  cleanup();
  mock.restoreAll();
});

after(() => dom.window.close());

function newClient() {
  return new QueryClient({ defaultOptions: { queries: { retry: false } } });
}

async function mount(client, { subscribe = false } = {}) {
  const { renderHook, waitFor } = await import("@testing-library/react");
  const view = renderHook(
    () => {
      useChannelSubscription(subscribe ? channel : null);
      return useChannelMessagesQuery(channel);
    },
    {
      wrapper: ({ children }) =>
        React.createElement(QueryClientProvider, { client }, children),
    },
  );
  // Track the fields the tests read: an observer re-renders only for fields
  // read before they change.
  void view.result.current.data;
  void view.result.current.isFetching;
  await waitFor(() => assert.equal(view.result.current.isSuccess, true));
  return view;
}

test("the proof rule: no gap, and live coverage or coverage through the last message", () => {
  const proof = { coveredThrough: 10, liveCovered: false, gap: false };
  assert.equal(isProvenStoreHead(proof, iso(10)), true);
  assert.equal(isProvenStoreHead(proof, iso(11)), false);
  assert.equal(isProvenStoreHead(proof, null), false);
  assert.equal(isProvenStoreHead({ ...proof, gap: true }, iso(5)), false);
  assert.equal(
    isProvenStoreHead(
      { ...proof, coveredThrough: null, liveCovered: true },
      null,
    ),
    true,
  );
  assert.equal(
    isProvenStoreHead({ ...proof, liveCovered: true, gap: true }, null),
    false,
  );
});

test("a store page maps summaries onto rows and keeps paging past a gap", () => {
  const summary = {
    id: "9".repeat(64),
    pubkey: "e".repeat(64),
    created_at: 20,
    kind: 39005,
    tags: [["e", root.id]],
    content: JSON.stringify({
      reply_count: 2,
      descendant_count: 3,
      last_reply_at: 19,
      participants: ["b".repeat(64)],
    }),
    sig: "",
  };
  const cursor = { createdAt: 10, eventId: root.id };
  const page = storeWindowPage(
    storedPage([root], { summaries: [summary], gap: true, nextCursor: cursor }),
    null,
  );
  assert.deepEqual(page.rows[0].thread, {
    replyCount: 2,
    descendantCount: 3,
    lastReplyAt: 19,
    participantPubkeys: ["b".repeat(64)],
  });
  assert.deepEqual(page.nextCursor, cursor);
  assert.equal(page.hasMore, true);
  assert.equal(storeWindowPage(storedPage(), null).hasMore, false);
});

test("a proven stored head paints on mount with no server request", async () => {
  install();
  const client = newClient();
  await hydrateChannelHeads(client, scope);
  assert.deepEqual(client.getQueryData(channelMessagesKey(channelId)), [root]);
  assert.equal(
    client.getQueryState(channelMessagesKey(channelId)).dataUpdatedAt,
    0,
  );
  assert.equal(hasPersistedHydratedChannel(client, channelId, iso(10)), true);
  const view = await mount(client);
  assert.deepEqual(view.result.current.data, [root]);
  assert.equal(serverCalls(), 0);
  view.unmount();
  client.clear();
});

test("a seeded head is consumed once", async () => {
  install();
  const client = newClient();
  await hydrateChannelHeads(client, scope);
  assert.equal(consumeHydratedChannel(client, channelId, iso(10)), true);
  assert.equal(consumeHydratedChannel(client, channelId, iso(10)), false);
  client.clear();
});

test("a stored head older than the room's last message is never painted", async () => {
  install();
  const client = newClient();
  await hydrateChannelHeads(client, scope);
  // The server knows a message newer than the stored coverage.
  assert.equal(hasPersistedHydratedChannel(client, channelId, iso(11)), false);
  client.clear();

  install({ coverage: { hiAt: 9 } });
  const stale = newClient();
  await hydrateChannelHeads(stale, scope);
  assert.equal(hasPersistedHydratedChannel(stale, channelId, iso(10)), false);
  const staleView = await mount(stale);
  console.log(
    "CALLS",
    calls.map((c) => c.command),
    JSON.stringify(staleView.result.current.data.map((e) => e.content)),
    staleView.result.current.fetchStatus,
  );
  const { waitFor } = await import("@testing-library/react");
  await waitFor(() =>
    assert.deepEqual(staleView.result.current.data, [replacement]),
  );
  assert.equal(serverCalls(), 1);
  staleView.unmount();
  stale.clear();
});

test("a head that stops at a coverage gap is not seeded", async () => {
  install({
    heads: [storedPage([root], { gap: true })],
    window: storedPage([root], { gap: true }),
  });
  const client = newClient();
  await hydrateChannelHeads(client, scope);
  assert.equal(client.getQueryData(channelMessagesKey(channelId)), undefined);
  assert.equal(
    calls.some((call) => call.command === "message_store_coverage"),
    false,
  );
  const view = await mount(client);
  assert.equal(serverCalls(), 1);
  assert.deepEqual(view.result.current.data, [replacement]);
  view.unmount();
  client.clear();
});

test("an unseeded room reads a proven head from the store", async () => {
  install({ heads: [] });
  const client = newClient();
  await hydrateChannelHeads(client, scope);
  const view = await mount(client);
  assert.deepEqual(view.result.current.data, [root]);
  assert.equal(serverCalls(), 0);
  view.unmount();
  client.clear();
});

test("a room mounted during a slow store load still takes the seeded path", async () => {
  install({ loadDelayMs: 150 });
  const client = newClient();
  void hydrateChannelHeads(client, scope);
  const view = await mount(client);
  assert.equal(serverCalls(), 0);
  assert.deepEqual(view.result.current.data, [root]);
  view.unmount();
  client.clear();
});

test("a subscribe refresh always asks the server", async () => {
  install();
  const client = newClient();
  await hydrateChannelHeads(client, scope);
  const view = await mount(client);
  await refreshChannelWindowMessages(client, channelId);
  // Coverage through the last message does not prove the interval before
  // the live subscription started.
  assert.equal(serverCalls(), 1);
  assert.deepEqual(client.getQueryData(channelMessagesKey(channelId)), [
    replacement,
  ]);

  // Live coverage does not either: the store does not sync thread summaries.
  store.coverage = { liveCovered: true };
  store.window = storedPage([replacement, root]);
  await refreshChannelWindowMessages(client, channelId);
  assert.equal(serverCalls(), 2);
  view.unmount();
  client.clear();
});

test("a seeded room still revalidates when live subscription setup fails", async () => {
  install();
  mock.method(relayClient, "subscribeToReconnects", () => () => {});
  mock.method(relayClient, "subscribeToChannelLive", () =>
    Promise.reject(new Error("socket down")),
  );
  mock.method(console, "error", () => {});
  const client = newClient();
  await hydrateChannelHeads(client, scope);
  const { waitFor } = await import("@testing-library/react");
  const view = await mount(client, { subscribe: true });
  await waitFor(() => assert.equal(serverCalls(), 1));
  await waitFor(() =>
    assert.deepEqual(view.result.current.data, [replacement]),
  );
  view.unmount();
  client.clear();
});

test("a live subscription that settles before a slow store load still revalidates", async () => {
  install({ loadDelayMs: 150 });
  mock.method(relayClient, "subscribeToReconnects", () => () => {});
  mock.method(relayClient, "subscribeToChannelLive", () =>
    Promise.resolve(async () => {}),
  );
  const client = newClient();
  void hydrateChannelHeads(client, scope);
  const { waitFor } = await import("@testing-library/react");
  const view = await mount(client, { subscribe: true });
  await waitFor(() => assert.equal(serverCalls(), 1));
  await waitFor(() =>
    assert.deepEqual(view.result.current.data, [replacement]),
  );
  view.unmount();
  client.clear();
});

test("a cold room with no store scope fetches the window once", async () => {
  install();
  mock.method(relayClient, "subscribeToReconnects", () => () => {});
  mock.method(relayClient, "subscribeToChannelLive", () =>
    Promise.resolve(async () => {}),
  );
  const client = newClient();
  const { waitFor } = await import("@testing-library/react");
  const view = await mount(client, { subscribe: true });
  await waitFor(() => assert.equal(view.result.current.isFetching, false));
  assert.equal(serverCalls(), 1);
  assert.deepEqual(view.result.current.data, [replacement]);
  view.unmount();
  client.clear();
});

test("older pages come from the store until its run ends, then from the server", async () => {
  const older = { ...root, id: "1".repeat(64), created_at: 5, content: "old" };
  const cursor = { createdAt: 10, eventId: root.id };
  install({
    window: ({ before }) =>
      before?.eventId === root.id
        ? storedPage([older], {
            startCursor: before,
            gap: true,
            nextCursor: { createdAt: 5, eventId: older.id },
            hasMore: true,
          })
        : storedPage([], { startCursor: before, gap: true }),
  });
  const client = newClient();
  await hydrateChannelHeads(client, scope);
  client.setQueryData(channelWindowKey(channelId), {
    pages: [
      {
        startCursor: null,
        rows: [{ event: root, thread: null }],
        aux: [],
        nextCursor: cursor,
        hasMore: true,
      },
    ],
    liveOverlay: [],
    liveAux: [],
    liveSummaries: {},
  });
  await pageOlderMessagesUntilRowFloor(client, channelId, () => true);
  assert.equal(serverCalls(), 0);
  assert.deepEqual(
    client
      .getQueryData(channelWindowKey(channelId))
      .pages[1].rows.map((row) => row.event.id),
    [older.id],
  );

  // The run ends below `older`: the next page is the server's.
  window.__TAURI_INTERNALS__.invoke = async (command, args) => {
    calls.push({ command, args });
    if (command === "message_store_read_window") {
      return storedPage([], { startCursor: args.before, gap: true });
    }
    if (command === "get_channel_window") {
      return [bounds(`5:${older.id}`)];
    }
    return null;
  };
  await pageOlderMessagesUntilRowFloor(client, channelId, () => true);
  assert.equal(serverCalls(), 1);
  client.clear();
});

test("a stored head keeps live thread summaries it cannot supersede", () => {
  const client = new QueryClient();
  const live = (createdAt) => ({
    createdAt,
    summary: {
      replyCount: 1,
      descendantCount: 1,
      lastReplyAt: createdAt,
      participantPubkeys: [],
    },
  });
  const summary = (createdAt) => ({
    id: "9".repeat(64),
    pubkey: "e".repeat(64),
    created_at: createdAt,
    kind: 39005,
    tags: [
      ["h", channelId],
      ["e", root.id],
    ],
    content: JSON.stringify({
      reply_count: 2,
      descendant_count: 2,
      last_reply_at: createdAt,
      participants: [],
    }),
    sig: "",
  });
  const other = "7".repeat(64);
  const seed = () =>
    client.setQueryData(channelWindowKey(channelId), {
      pages: [],
      liveOverlay: [],
      liveAux: [],
      liveSummaries: { [root.id]: live(20), [other]: live(20) },
    });
  const liveRoots = () =>
    Object.keys(
      client.getQueryData(channelWindowKey(channelId)).liveSummaries,
    ).sort();

  // The store keeps no live summaries: an older stored one supersedes none.
  seed();
  applyStoreHead(
    client,
    channelId,
    storeWindowPage(storedPage([root], { summaries: [summary(19)] }), null),
  );
  assert.deepEqual(liveRoots(), [root.id, other].sort());

  // A stored summary at least as new as the live one replaces it.
  seed();
  applyStoreHead(
    client,
    channelId,
    storeWindowPage(storedPage([root], { summaries: [summary(20)] }), null),
  );
  assert.deepEqual(liveRoots(), [other]);
  client.clear();
});
