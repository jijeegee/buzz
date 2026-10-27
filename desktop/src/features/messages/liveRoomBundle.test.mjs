// Open room + thread fed by the shared live room bundle: real hooks and
// RelayClient bundle, mocked relay subscription and Tauri IPC.
import assert from "node:assert/strict";
import { after, afterEach, before, mock, test } from "node:test";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { JSDOM } from "jsdom";
import React from "react";

import { useChannelMessagesQuery, useChannelSubscription } from "./hooks.ts";
import {
  channelMessagesKey,
  channelWindowKey,
  threadRepliesKey,
} from "./lib/messageQueryKeys.ts";
import { channelWindowThreadSummaries } from "./lib/channelWindowStore.ts";
import { relayClient } from "../../shared/api/relayClient.ts";

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
afterEach(async () => {
  const { cleanup } = await import("@testing-library/react");
  cleanup();
  for (const client of clients.splice(0)) client.clear();
  mock.restoreAll();
  relayClient.disconnect();
});
after(() => dom.window.close());

const roomA = "room-a";
const roomB = "room-b";
const AUTHOR = "b".repeat(64);
const channel = {
  id: roomA,
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
const id = (char) => char.repeat(64);
const root = {
  id: id("1"),
  pubkey: AUTHOR,
  created_at: 100,
  kind: 9,
  tags: [["h", roomA]],
  content: "root",
  sig: "",
};
const bounds = {
  id: id("9"),
  pubkey: id("e"),
  created_at: 101,
  kind: 39006,
  tags: [
    ["h", roomA],
    ["d", `${roomA}:head`],
  ],
  content: JSON.stringify({ has_more: false, next_cursor: null }),
  sig: "",
};
const reply = (eventId, extraTags = []) => ({
  id: eventId,
  pubkey: AUTHOR,
  created_at: 200,
  kind: 9,
  tags: [
    ["h", roomA],
    ["e", root.id, "", "root"],
    ["e", root.id, "", "reply"],
    ...extraTags,
  ],
  content: "reply",
  sig: "",
});

async function mountRoom() {
  window.localStorage.clear();
  window.__TAURI_INTERNALS__ = {
    invoke: async (command) => {
      if (command === "get_channel_window") return [root, bounds];
      return null;
    },
  };
  mock.method(relayClient, "subscribeToReconnects", () => () => {});
  const bundles = [];
  mock.method(relayClient, "subscribeLive", async (filter, onEvent) => {
    const sub = { filter, onEvent, disposed: false };
    bundles.push(sub);
    return async () => {
      sub.disposed = true;
    };
  });
  relayClient.liveRooms.setMemberRooms([roomA, roomB]);

  const { act, renderHook, waitFor } = await import("@testing-library/react");
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  clients.push(client);
  const view = renderHook(
    () => {
      useChannelSubscription(channel);
      return useChannelMessagesQuery(channel);
    },
    {
      wrapper: ({ children }) =>
        React.createElement(QueryClientProvider, { client }, children),
    },
  );
  await waitFor(() =>
    assert.ok(view.result.current.data?.some((event) => event.id === root.id)),
  );
  assert.equal(bundles.length, 1, "the open room rides the member bundle");
  assert.deepEqual(bundles[0].filter["#h"], [roomA, roomB]);
  const deliver = (event) =>
    act(async () => {
      bundles[0].onEvent(event);
    });
  return { client, view, deliver, bundles, waitFor };
}

const threadKey = threadRepliesKey(roomA, root.id);
const ids = (events) => (events ?? []).map((event) => event.id);

test("an open thread draws live replies, their reactions and removals from the bundle", async () => {
  const { client, deliver } = await mountRoom();
  // The thread panel is open: its cache exists.
  client.setQueryData(threadKey, []);

  await deliver(reply(id("2")));
  assert.deepEqual(ids(client.getQueryData(threadKey)), [id("2")]);
  assert.ok(
    !ids(client.getQueryData(channelMessagesKey(roomA))).includes(id("2")),
    "a plain reply stays out of the main timeline",
  );

  // Reactions carry no h tag: the bundle places them by the reply they name.
  const reaction = {
    id: id("3"),
    pubkey: AUTHOR,
    created_at: 201,
    kind: 7,
    tags: [["e", id("2")]],
    content: "👍",
    sig: "",
  };
  await deliver(reaction);
  assert.ok(ids(client.getQueryData(threadKey)).includes(id("3")));

  const unreact = {
    id: id("4"),
    pubkey: AUTHOR,
    created_at: 202,
    kind: 5,
    tags: [["e", id("3")]],
    content: "",
    sig: "",
  };
  await deliver(unreact);
  assert.ok(ids(client.getQueryData(threadKey)).includes(id("4")));
});

test("a broadcast reply reaches both the thread and the main timeline", async () => {
  const { client, deliver } = await mountRoom();
  client.setQueryData(threadKey, []);
  await deliver(reply(id("5"), [["broadcast", "1"]]));
  assert.deepEqual(ids(client.getQueryData(threadKey)), [id("5")]);
  assert.ok(
    ids(client.getQueryData(channelMessagesKey(roomA))).includes(id("5")),
  );
});

test("a live thread summary updates the room's reply badge, and another room's events stay out", async () => {
  const { client, deliver } = await mountRoom();
  client.setQueryData(threadKey, []);
  await deliver({
    id: id("6"),
    pubkey: id("e"),
    created_at: 300,
    kind: 39005,
    tags: [
      ["e", root.id],
      ["d", root.id],
      ["h", roomA],
    ],
    content: JSON.stringify({
      reply_count: 3,
      descendant_count: 3,
      last_reply_at: 299,
      participants: [AUTHOR],
    }),
    sig: "",
  });
  const summaries = channelWindowThreadSummaries(
    client.getQueryData(channelWindowKey(roomA)),
  );
  assert.equal(summaries.get(root.id)?.replyCount, 3);

  const before = ids(client.getQueryData(channelMessagesKey(roomA)));
  await deliver({
    ...root,
    id: id("7"),
    created_at: 301,
    tags: [["h", roomB]],
    content: "elsewhere",
  });
  await deliver({
    ...reply(id("8")),
    tags: [
      ["h", roomB],
      ["e", root.id, "", "root"],
      ["e", root.id, "", "reply"],
    ],
  });
  assert.deepEqual(ids(client.getQueryData(channelMessagesKey(roomA))), before);
  assert.deepEqual(ids(client.getQueryData(threadKey)), []);
});

test("a new top-level message in the open room is drawn live", async () => {
  const { client, deliver, waitFor } = await mountRoom();
  await deliver({ ...root, id: id("a"), created_at: 400, content: "new" });
  await waitFor(() =>
    assert.ok(
      ids(client.getQueryData(channelMessagesKey(roomA))).includes(id("a")),
    ),
  );
});
