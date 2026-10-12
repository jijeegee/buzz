import assert from "node:assert/strict";
import test from "node:test";

import { buildInboxItems } from "./inbox.ts";
import { inboxSummaryFeedItems } from "./inboxSummary.ts";
import {
  collapseChannelMainRows,
  roomRootsWithoutMainRows,
} from "./inboxViewHelpers.ts";

const OTHER = "b".repeat(64);
const ROOM = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
const ROOT = "c".repeat(64);

function event(id, createdAt, tags = []) {
  return {
    content: `message ${id}`,
    created_at: createdAt,
    id,
    kind: 9,
    pubkey: OTHER,
    tags: [["h", ROOM], ...tags],
  };
}

const rootEvent = event(ROOT, 100);
// The relay's room row carries its newest top-level message: here the root.
const rows = [
  {
    at: 200,
    at_us: 200_000_000,
    channel_id: ROOM,
    channel_type: "stream",
    key: ROOT,
    latest: event("reply-1", 200, [["e", ROOT, "", "reply"]]),
    reply_count: 1,
    root: rootEvent,
    root_id: ROOT,
    type: "thread",
    unread: 1,
    unread_overflow: false,
  },
  {
    at: 100,
    at_us: 100_000_000,
    channel_id: ROOM,
    channel_type: "stream",
    key: ROOM,
    latest: rootEvent,
    reply_count: null,
    root: null,
    root_id: null,
    type: "room",
    unread: 1,
    unread_overflow: false,
  },
];

const channels = [{ channelType: "stream", id: ROOM, name: "general" }];

function items(feed) {
  return buildInboxItems({
    channels,
    currentPubkey: "a".repeat(64),
    feed: {
      feed: {
        activity: feed,
        agentActivity: [],
        mentions: [],
        needsAction: [],
      },
      meta: { generatedAt: 0, since: 0, total: feed.length },
    },
    getChannelReadAt: () => null,
    getMessageReadAt: () => null,
    getThreadReadAt: () => null,
  });
}

test("a room whose latest message is a thread root keeps its own row", () => {
  const channelById = new Map(channels.map((c) => [c.id, c]));
  const inboxItems = items(inboxSummaryFeedItems(rows, channelById));
  // Without the fix the root folds into the thread and the room row is lost.
  assert.deepEqual(
    collapseChannelMainRows(inboxItems).map((row) => row.conversationId),
    [ROOT],
  );

  const roots = roomRootsWithoutMainRows(inboxItems);
  assert.deepEqual(
    roots.map((root) => root.id),
    [ROOT],
  );
  const merged = [...inboxItems, ...items(roots)].sort(
    (left, right) => right.latestActivityAt - left.latestActivityAt,
  );
  assert.deepEqual(
    collapseChannelMainRows(merged).map((row) => row.conversationId),
    [ROOT, `channel:${ROOM}`],
  );
});

test("a room with its own main-timeline row is left alone", () => {
  const channelById = new Map(channels.map((c) => [c.id, c]));
  const withMain = [
    ...rows.slice(0, 1),
    { ...rows[1], latest: event("top-level", 150) },
  ];
  const inboxItems = items(inboxSummaryFeedItems(withMain, channelById));
  assert.deepEqual(roomRootsWithoutMainRows(inboxItems), []);
});
