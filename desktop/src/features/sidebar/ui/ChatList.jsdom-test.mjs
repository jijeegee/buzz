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
