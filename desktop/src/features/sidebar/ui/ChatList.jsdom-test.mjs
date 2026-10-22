import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { SidebarProvider } from "@/shared/ui/sidebar";
import { ChatList } from "./ChatList.tsx";
import { applyChannelLastMessageAt } from "@/features/channels/lib/channelRecency";

window.matchMedia = () => ({
  matches: false,
  addEventListener() {},
  removeEventListener() {},
});

function channel(id, overrides = {}) {
  return {
    id,
    name: id,
    channelType: "stream",
    visibility: "public",
    isMember: true,
    archivedAt: null,
    participantPubkeys: [],
    participants: [],
    memberPubkeys: [],
    lastMessageAt: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

test("rendered mixed chats use pins, actual message time and stable IDs; preserve visibility and navigation", async () => {
  const client = new QueryClient({
    defaultOptions: {
      queries: { enabled: false, retry: false, gcTime: Infinity },
    },
  });
  client.setQueryData(["identity"], { pubkey: "owner" });
  let items = [
    channel("private", { visibility: "private" }),
    channel("dm", {
      channelType: "dm",
      isMember: false,
      lastMessageAt: "2026-01-03T00:00:00Z",
    }),
    channel("public", { lastMessageAt: "2026-01-02T00:00:00Z" }),
    channel("empty", { lastMessageAt: null }),
    channel("unjoined", { isMember: false }),
    channel("archived", { archivedAt: "2026-01-01" }),
    channel("forum", { channelType: "forum" }),
  ];
  const selected = [];
  const hidden = [];
  const props = {
    isActiveChannel: false,
    selectedChannelId: null,
    unreadChannelIds: new Set(["dm"]),
    unreadChannelCounts: new Map([["dm", 3]]),
    mutedChannelIds: new Set(["private"]),
    channelLabels: { dm: "Alice" },
    onSelectChannel: (id) => selected.push(id),
    onHideDm: (id) => hidden.push(id),
  };
  const tree = (pins = new Set()) =>
    React.createElement(
      QueryClientProvider,
      { client },
      React.createElement(
        SidebarProvider,
        null,
        React.createElement(ChatList, {
          ...props,
          items,
          starredChannelIds: pins,
        }),
      ),
    );
  let view;
  try {
    await act(async () => {
      view = render(tree());
    });
    const order = () =>
      [
        ...view.getByTestId("chat-list").querySelectorAll("[data-channel-id]"),
      ].map((row) => row.dataset.channelId);
    assert.deepEqual(order(), ["dm", "public", "private", "empty"]);
    assert.match(view.getByTestId("channel-dm").textContent, /Alice/);
    assert.match(view.getByTestId("channel-dm").parentElement.textContent, /3/);
    assert.ok(
      view.getByTestId("channel-private").querySelector(".lucide-lock"),
    );
    assert.ok(
      view.getByTestId("channel-private").querySelector(".lucide-bell-off"),
    );
    fireEvent.click(view.getByTestId("channel-public"));
    fireEvent.click(view.getByTestId("hide-dm-dm"));
    assert.deepEqual(selected, ["public"]);
    assert.deepEqual(hidden, ["dm"]);

    view.rerender(tree(new Set(["private", "dm"])));
    assert.deepEqual(order(), ["dm", "private", "public", "empty"]);
    assert.match(
      view.getByTestId("chat-pinned-private").textContent,
      /Pinned for you/,
    );
    // An empty pinned room still precedes every unpinned active room.
    view.rerender(tree(new Set(["empty"])));
    assert.deepEqual(order(), ["empty", "dm", "public", "private"]);
    view.rerender(tree());
    assert.deepEqual(order(), ["dm", "public", "private", "empty"]);

    items = applyChannelLastMessageAt(items, "private", "2026-01-04T00:00:00Z");
    view.rerender(tree());
    assert.deepEqual(order(), ["private", "dm", "public", "empty"]);
    items = items.map((item) => ({
      ...item,
      name: `Renamed ${item.name}`,
      updatedAt: "2030-01-01",
      topic: "Changed settings",
    }));
    view.rerender(tree());
    assert.deepEqual(order(), ["private", "dm", "public", "empty"]);

    items = [
      channel("b", { name: "Alpha" }),
      channel("a", { name: "Zulu" }),
      channel("z", { lastMessageAt: "invalid" }),
    ];
    view.rerender(tree());
    assert.deepEqual(order(), ["a", "b", "z"]);
    items = [...items]
      .reverse()
      .map((item) => ({ ...item, name: item.id === "a" ? "ZZZ" : "AAA" }));
    view.rerender(tree());
    assert.deepEqual(order(), ["a", "b", "z"]);
  } finally {
    cleanup();
    client.clear();
  }
});

const ROOT = "c".repeat(64);
const PINNED_ROOT = "d".repeat(64);

function serverRow(overrides) {
  const type = overrides.type ?? "room";
  const at = overrides.at ?? 100;
  return {
    type,
    key: type === "thread" ? overrides.root_id : overrides.channel_id,
    at,
    at_us: at * 1_000_000,
    channel_type: "stream",
    channel_name: overrides.channel_id,
    archived: false,
    dm_members: [],
    root_id: null,
    thread_name: null,
    follow_state: type === "thread" ? "auto" : null,
    closed: false,
    last_seq: 1,
    read_seq: 0,
    unread: 0,
    unread_overflow: false,
    unread_mentions: 0,
    unread_mentions_overflow: false,
    unread_mark: false,
    latest: {
      id: `latest-${overrides.key ?? overrides.root_id ?? overrides.channel_id}`,
      pubkey: "other",
      created_at: at,
      kind: 9,
      tags: [],
      content: "hi",
    },
    root: null,
    ...overrides,
  };
}

test("with server chats on, rows, threads and counts come from the relay; off keeps today's list", async () => {
  const client = new QueryClient({
    defaultOptions: {
      queries: { enabled: false, retry: false, gcTime: Infinity },
    },
  });
  client.setQueryData(["identity"], { pubkey: "owner" });
  const items = [
    channel("room-a", { lastMessageAt: "2026-01-01T00:00:00Z" }),
    channel("dm-b", {
      channelType: "dm",
      lastMessageAt: "2026-01-02T00:00:00Z",
    }),
    channel("room-c", { lastMessageAt: "2026-01-05T00:00:00Z" }),
    channel("forum", { channelType: "forum" }),
  ];
  const opened = [];
  let readAt = {};
  const serverRows = [
    serverRow({
      type: "thread",
      channel_id: "room-a",
      root_id: ROOT,
      thread_name: "Launch plan",
      at: 300,
      unread: 2,
      unread_mentions: 1,
    }),
    serverRow({ channel_id: "dm-b", channel_type: "dm", at: 200, unread: 4 }),
    serverRow({ channel_id: "room-a", at: 100 }),
    // Not in the local list (left, or not loaded yet): not rendered.
    serverRow({ channel_id: "gone", at: 400, unread: 9 }),
    serverRow({
      type: "thread",
      channel_id: "room-a",
      root_id: PINNED_ROOT,
      at: 50,
      closed: true,
      root: {
        id: PINNED_ROOT,
        pubkey: "other",
        created_at: 10,
        kind: 9,
        tags: [],
        content: "Old question\nwith details",
      },
    }),
  ];
  const tree = (serverChats) =>
    React.createElement(
      QueryClientProvider,
      { client },
      React.createElement(
        SidebarProvider,
        null,
        React.createElement(ChatList, {
          isActiveChannel: false,
          selectedChannelId: null,
          items,
          // Local counts that the server list must not use.
          unreadChannelIds: new Set(["dm-b"]),
          unreadChannelCounts: new Map([["dm-b", 1]]),
          channelLabels: { "dm-b": "Bob" },
          onSelectChannel: () => {},
          serverChats,
        }),
      ),
    );
  const server = (version = 0) => ({
    rows: serverRows,
    getReadAt: (contextId) => readAt[contextId] ?? null,
    onOpenThread: (channelId, rootId) => opened.push([channelId, rootId]),
    readStateVersion: version,
    pinnedThreadIds: new Set([PINNED_ROOT]),
  });
  const order = (view) =>
    [
      ...view
        .getByTestId("chat-list")
        .querySelectorAll("[data-channel-id],[data-chat-thread-id]"),
    ].map(
      (row) => row.dataset.channelId ?? `thread:${row.dataset.chatThreadId}`,
    );
  let view;
  try {
    await act(async () => {
      view = render(tree(server()));
    });
    assert.deepEqual(order(view), [
      `thread:${PINNED_ROOT}`,
      `thread:${ROOT}`,
      "dm-b",
      "room-a",
      "room-c",
    ]);
    const thread = view.getByTestId(`chat-thread-${ROOT}`);
    assert.match(thread.textContent, /room-a/);
    assert.match(thread.textContent, /Launch plan/);
    assert.match(
      view.getByTestId(`chat-thread-unread-${ROOT}`).textContent,
      /^2/,
    );
    assert.ok(view.getByTestId(`chat-thread-mentions-${ROOT}`));
    const pinned = view.getByTestId(`chat-thread-${PINNED_ROOT}`);
    assert.match(pinned.textContent, /Old question/);
    assert.match(pinned.textContent, /Pinned for you/);
    assert.ok(view.getByTestId(`chat-thread-closed-${PINNED_ROOT}`));
    // Server count for the DM, not the local one.
    assert.match(view.getByTestId("channel-unread-dm-b").textContent, /^4/);
    // The row's full-width button opens it.
    fireEvent.click(thread.querySelector("button"));
    assert.deepEqual(opened, [["room-a", ROOT]]);

    // This device read the DM past its latest message: the count goes away
    // before the relay catches up.
    readAt = { "dm-b": 200 };
    view.rerender(tree(server(1)));
    assert.equal(view.queryByTestId("channel-unread-dm-b"), null);

    // Marked unread on this device: unread on top of the server row.
    const dmRow = () =>
      view.getByTestId("chat-list").querySelector('[data-channel-id="dm-b"]');
    // Unread shows as a bold latest message, as in the Inbox.
    const dmPreview = () => dmRow().querySelector("p").className;
    assert.match(dmPreview(), /font-normal/);
    view.rerender(
      tree({ ...server(2), forcedUnreadChannelIds: new Set(["dm-b"]) }),
    );
    assert.match(dmPreview(), /font-semibold/);

    // A thread muted (unfollowed) on this device stays out of the list.
    view.rerender(tree({ ...server(3), mutedRootIds: new Set([ROOT]) }));
    assert.equal(view.queryByTestId(`chat-thread-${ROOT}`), null);
    assert.ok(view.getByTestId(`chat-thread-${PINNED_ROOT}`));

    // Switch off (or unknown): exactly today's list and local counts.
    view.rerender(tree(undefined));
    assert.deepEqual(order(view), ["room-c", "dm-b", "room-a"]);
    assert.equal(view.queryByTestId(`chat-thread-${ROOT}`), null);
    assert.match(view.getByTestId("channel-unread-dm-b").textContent, /^1/);
  } finally {
    cleanup();
    client.clear();
  }
});

test("By channel groups threads under their channel and expands in place", async () => {
  const client = new QueryClient({
    defaultOptions: {
      queries: { enabled: false, retry: false, gcTime: Infinity },
    },
  });
  client.setQueryData(["identity"], { pubkey: "owner" });
  const items = [channel("room-a"), channel("room-b")];
  const roots = ["1", "2", "3", "4", "5"].map((n) => n.repeat(64));
  const serverRows = [
    serverRow({ channel_id: "room-b", at: 900 }),
    ...roots.map((root, index) =>
      serverRow({
        type: "thread",
        channel_id: "room-a",
        root_id: root,
        thread_name: `T${index + 1}`,
        at: 800 - index * 10,
        unread: index >= 3 ? 2 : 0,
        unread_mentions: index === 4 ? 1 : 0,
      }),
    ),
    serverRow({ channel_id: "room-a", at: 100 }),
  ];
  const tree = (view, rows = serverRows) =>
    React.createElement(
      QueryClientProvider,
      { client },
      React.createElement(
        SidebarProvider,
        null,
        React.createElement(ChatList, {
          isActiveChannel: false,
          selectedChannelId: null,
          items,
          unreadChannelIds: new Set(),
          unreadChannelCounts: new Map(),
          onSelectChannel: () => {},
          serverChats: {
            rows,
            getReadAt: () => null,
            onOpenThread: () => {},
            readStateVersion: 0,
          },
          view,
        }),
      ),
    );
  const order = (view) =>
    [
      ...view
        .getByTestId("chat-list")
        .querySelectorAll(
          "[data-channel-id],[data-chat-thread-id],[data-testid^='chat-more-threads-room'],[data-testid^='chat-more-threads-less-']",
        ),
    ].map((row) =>
      row.dataset.channelId
        ? row.dataset.channelId
        : row.dataset.chatThreadId
          ? row.dataset.chatThreadId[0]
          : row.textContent,
    );
  let view;
  try {
    await act(async () => {
      view = render(tree("by-channel"));
    });
    // room-b's main row is newer than any of room-a's rows.
    assert.deepEqual(order(view), [
      "room-b",
      "room-a",
      "1",
      "2",
      "3",
      "+ 2 more threads@ 1 unread mention4 unread messages",
    ]);
    fireEvent.click(view.getByTestId("chat-more-threads-room-a"));
    assert.deepEqual(order(view), [
      "room-b",
      "room-a",
      "1",
      "2",
      "3",
      "4",
      "5",
      "Show less",
    ]);
    fireEvent.click(view.getByTestId("chat-more-threads-less-room-a"));
    assert.equal(order(view).length, 6);
    // All chats: every row by activity, no grouping.
    // room-a's main row is not on a loaded page: the local row heads the
    // group, once.
    view.rerender(
      tree(
        "by-channel",
        serverRows.filter(
          (row) => !(row.type === "room" && row.channel_id === "room-a"),
        ),
      ),
    );
    assert.deepEqual(order(view), [
      "room-b",
      "room-a",
      "1",
      "2",
      "3",
      "+ 2 more threads@ 1 unread mention4 unread messages",
    ]);
    view.rerender(tree("all"));
    assert.deepEqual(order(view), [
      "room-b",
      "1",
      "2",
      "3",
      "4",
      "5",
      "room-a",
    ]);
  } finally {
    cleanup();
    client.clear();
  }
});

test("By channel reveals 10 more threads per click, then Show less returns to 3", async () => {
  const client = new QueryClient({
    defaultOptions: {
      queries: { enabled: false, retry: false, gcTime: Infinity },
    },
  });
  client.setQueryData(["identity"], { pubkey: "owner" });
  // 28 threads; only the oldest one has unread (3) and a mention.
  const roots = Array.from({ length: 28 }, (_, index) =>
    index.toString(16).padStart(64, "0"),
  );
  const rows = [
    serverRow({ channel_id: "room-a", at: 900 }),
    ...roots.map((root, index) =>
      serverRow({
        type: "thread",
        channel_id: "room-a",
        root_id: root,
        thread_name: `T${index + 1}`,
        at: 800 - index * 10,
        unread: index === 27 ? 3 : 0,
        unread_mentions: index === 27 ? 1 : 0,
      }),
    ),
  ];
  const tree = () =>
    React.createElement(
      QueryClientProvider,
      { client },
      React.createElement(
        SidebarProvider,
        null,
        React.createElement(ChatList, {
          isActiveChannel: false,
          selectedChannelId: null,
          items: [channel("room-a")],
          unreadChannelIds: new Set(),
          unreadChannelCounts: new Map(),
          onSelectChannel: () => {},
          serverChats: {
            rows,
            getReadAt: () => null,
            onOpenThread: () => {},
            readStateVersion: 0,
          },
          view: "by-channel",
        }),
      ),
    );
  const threads = (view) =>
    view.getByTestId("chat-list").querySelectorAll("[data-chat-thread-id]")
      .length;
  const more = (view) => view.queryByTestId("chat-more-threads-room-a");
  const less = (view) => view.queryByTestId("chat-more-threads-less-room-a");
  let view;
  try {
    await act(async () => {
      view = render(tree());
    });
    assert.equal(threads(view), 3);
    assert.equal(
      more(view).textContent,
      "+ 25 more threads@ 1 unread mention3 unread messages",
    );
    assert.equal(less(view), null);
    fireEvent.click(more(view));
    assert.equal(threads(view), 13);
    assert.equal(
      more(view).textContent,
      "+ 10 more · 15 left@ 1 unread mention3 unread messages",
    );
    assert.equal(less(view).textContent, "Show less");
    fireEvent.click(more(view));
    assert.equal(threads(view), 23);
    assert.equal(
      more(view).textContent,
      "+ 5 more@ 1 unread mention3 unread messages",
    );
    fireEvent.click(more(view));
    assert.equal(threads(view), 28);
    assert.equal(more(view), null);
    assert.equal(less(view).textContent, "Show less");
    fireEvent.click(less(view));
    assert.equal(threads(view), 3);
    assert.equal(less(view), null);
    assert.match(more(view).textContent, /^\+ 25 more threads/);
  } finally {
    cleanup();
    client.clear();
  }
});
