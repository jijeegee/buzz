import assert from "node:assert/strict";
import test from "node:test";

import {
  groupChatRowsByChannel,
  parseChatsViewMode,
  readChatsViewMode,
  writeChatsViewMode,
} from "./chatListGroups.ts";

function room(channel, at) {
  return {
    type: "room",
    key: channel,
    channel_id: channel,
    root_id: null,
    at,
    at_us: at * 1e6,
    unread: 0,
  };
}
function thread(channel, root, at, unread = 0, mentions = 0) {
  return {
    type: "thread",
    key: root,
    channel_id: channel,
    root_id: root,
    at,
    at_us: at * 1e6,
    unread,
    unread_mentions: mentions,
  };
}
const unreadOf = (row) => ({
  count: row.unread ?? 0,
  overflow: false,
  mentions: row.unread_mentions ?? 0,
  mentionsOverflow: false,
  marked: false,
});
const none = () => false;

function shape({ entries }) {
  return entries.map((entry) =>
    entry.kind === "row"
      ? entry.row.key
      : entry.kind === "channel"
        ? `channel:${entry.channelId}`
        : `more:${entry.channelId}:${entry.count}:${entry.expanded ? "open" : "shut"}:${entry.unread}:${entry.mentions}`,
  );
}

test("a channel shows its main row and up to 3 newest threads, then a summed more line", () => {
  const rows = [
    thread("c1", "t5", 50, 2),
    thread("c1", "t4", 40),
    room("c1", 35),
    thread("c1", "t3", 30, 1, 1),
    thread("c1", "t2", 20, 4),
    thread("c1", "t1", 10, 0, 2),
  ];
  const entries = groupChatRowsByChannel(rows, {
    expandedChannelIds: new Set(),
    isPinnedRoom: none,
    isPinnedThread: none,
    unreadOf,
  });
  assert.deepEqual(shape(entries), [
    "c1",
    "t5",
    "t4",
    "t3",
    "more:c1:2:shut:4:2",
  ]);
});

test("expanding lists every thread and ends with Show less", () => {
  const rows = [
    room("c1", 60),
    ...[5, 4, 3, 2].map((n) => thread("c1", `t${n}`, n * 10, 1)),
  ];
  const entries = groupChatRowsByChannel(rows, {
    expandedChannelIds: new Set(["c1"]),
    isPinnedRoom: none,
    isPinnedThread: none,
    unreadOf,
  });
  assert.deepEqual(shape(entries), [
    "c1",
    "t5",
    "t4",
    "t3",
    "t2",
    "more:c1:1:open:0:0",
  ]);
});

test("groups keep the order of their newest row; DMs are groups too", () => {
  const rows = [
    thread("c2", "x1", 90),
    room("dm1", 80),
    room("c1", 70),
    room("c2", 60),
    thread("dm1", "d1", 50),
  ];
  const entries = groupChatRowsByChannel(rows, {
    expandedChannelIds: new Set(),
    isPinnedRoom: none,
    isPinnedThread: none,
    unreadOf,
  });
  assert.deepEqual(shape(entries), ["c2", "x1", "dm1", "d1", "c1"]);
});

test("a pinned thread pins its group to the top and leads inside it", () => {
  const rows = [
    room("c1", 90),
    room("c2", 80),
    thread("c2", "a", 70),
    thread("c2", "b", 60),
    thread("c2", "c", 50),
    thread("c2", "pinned", 10),
  ];
  const entries = groupChatRowsByChannel(rows, {
    expandedChannelIds: new Set(),
    isPinnedRoom: none,
    isPinnedThread: (root) => root === "pinned",
    unreadOf,
  });
  assert.deepEqual(shape(entries), [
    "c2",
    "pinned",
    "a",
    "b",
    "more:c2:1:shut:0:0",
    "c1",
  ]);
  assert.equal(entries.pinnedEntries, 5);
});

test("a group whose main row is not loaded starts with the channel heading", () => {
  const entries = groupChatRowsByChannel(
    [room("c1", 9), thread("c9", "t", 5)],
    {
      expandedChannelIds: new Set(),
      isPinnedRoom: (id) => id === "c9",
      isPinnedThread: none,
      unreadOf,
    },
  );
  // c9 is pinned locally, so its group leads even without its main row.
  assert.deepEqual(shape(entries), ["channel:c9", "t", "c1"]);
  assert.equal(entries.pinnedEntries, 2);
});

test("the view choice is stored per device and defaults to All chats", () => {
  const store = new Map();
  const storage = {
    getItem: (key) => store.get(key) ?? null,
    setItem: (key, value) => store.set(key, value),
  };
  assert.equal(readChatsViewMode(storage), "all");
  writeChatsViewMode(storage, "by-channel");
  assert.equal(readChatsViewMode(storage), "by-channel");
  assert.equal(parseChatsViewMode("bogus"), "all");
  assert.equal(readChatsViewMode(undefined), "all");
});
