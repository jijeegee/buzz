import assert from "node:assert/strict";
import test from "node:test";

import {
  applyLiveEventToInboxSummary,
  deletedEventIds,
  inboxSummaryConversationId,
  inboxSummaryFeedItems,
  inboxRowReadAt,
  inboxSummaryShowsAny,
  inboxSummaryUnread,
  mergeInboxSummaryPage,
  withServerUnreadCounts,
} from "./inboxSummary.ts";

const ME = "a".repeat(64);
const OTHER = "b".repeat(64);
const ROOM = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
const QUIET = "8ad375a7-6990-4b22-985f-e3fd34f634d7";
const ROOT = "c".repeat(64);

function event(id, createdAt, tags = [], pubkey = OTHER) {
  return {
    content: `message ${id}`,
    created_at: createdAt,
    id,
    kind: 9,
    pubkey,
    tags: [["h", ROOM], ...tags],
  };
}

function room(channelId, at, unread = 0) {
  return {
    at,
    at_us: at * 1_000_000,
    channel_id: channelId,
    channel_type: "stream",
    key: channelId,
    latest: event(`${channelId}-latest`, at),
    reply_count: null,
    root: null,
    root_id: null,
    type: "room",
    unread,
    unread_overflow: false,
  };
}

function thread(at, unread = 0) {
  return {
    at,
    at_us: at * 1_000_000,
    channel_id: ROOM,
    channel_type: "stream",
    key: ROOT,
    latest: event("reply-1", at, [["e", ROOT, "", "reply"]]),
    reply_count: 1,
    root: event(ROOT, at - 10),
    root_id: ROOT,
    type: "thread",
    unread,
    unread_overflow: false,
  };
}

const state = (rows) => ({
  generation: 1,
  next: null,
  rows,
  sentReadMarkers: true,
});
const channel = { channelType: "stream", id: ROOM };

test("a main-timeline message moves only its room row to the top", () => {
  const current = state([room(QUIET, 200), thread(150), room(ROOM, 100)]);
  const next = applyLiveEventToInboxSummary(
    current,
    event("new", 300),
    channel,
    ME,
  );
  assert.deepEqual(
    next.rows.map((row) => row.key),
    [ROOM, QUIET, ROOT],
  );
  assert.equal(next.rows[0].latest.id, "new");
  assert.equal(next.rows[0].unread, 1);
  assert.equal(next.rows[1], current.rows[0], "untouched rows keep identity");
});

test("a thread reply updates the thread row, not the room preview", () => {
  const current = state([room(ROOM, 200), thread(150, 2)]);
  const next = applyLiveEventToInboxSummary(
    current,
    event("reply-2", 300, [["e", ROOT, "", "reply"]]),
    channel,
    ME,
  );
  assert.equal(next.rows[0].key, ROOT);
  assert.equal(next.rows[0].reply_count, 2);
  assert.equal(next.rows[0].unread, 3);
  assert.equal(next.rows[1], current.rows[0]);
});

test("own messages do not count as unread and duplicates are ignored", () => {
  const current = state([room(ROOM, 100, 4)]);
  const own = event("mine", 200, [], ME);
  const once = applyLiveEventToInboxSummary(current, own, channel, ME);
  assert.equal(once.rows[0].unread, 4);
  assert.equal(applyLiveEventToInboxSummary(once, own, channel, ME), once);
});

test("other kinds and stale events leave the summary untouched", () => {
  const current = state([room(ROOM, 300)]);
  assert.equal(
    applyLiveEventToInboxSummary(
      current,
      { ...event("reaction", 400), kind: 7 },
      channel,
      ME,
    ),
    current,
  );
  assert.equal(
    applyLiveEventToInboxSummary(current, event("old", 200), channel, ME),
    current,
  );
});

test("a reply to an unknown thread inserts a new thread row", () => {
  const next = applyLiveEventToInboxSummary(
    state([room(ROOM, 100)]),
    event("reply", 200, [["e", "d".repeat(64), "", "reply"]]),
    channel,
    ME,
  );
  assert.equal(next.rows[0].type, "thread");
  assert.equal(next.rows[0].root_id, "d".repeat(64));
  assert.equal(next.rows[0].unread, null);
});

test("rows map to Inbox conversation ids and feed items", () => {
  const dm = { ...room(QUIET, 100, 2), channel_type: "dm" };
  assert.equal(inboxSummaryConversationId(room(ROOM, 1)), `channel:${ROOM}`);
  assert.equal(inboxSummaryConversationId(thread(1)), ROOT);
  assert.equal(inboxSummaryConversationId(dm), `dm:${QUIET}`);
  const items = inboxSummaryFeedItems(
    [thread(150)],
    new Map([[ROOM, { channelType: "stream", id: ROOM, name: "kuzz" }]]),
  );
  assert.deepEqual(
    items.map((item) => [item.id, item.channelName, item.category]),
    [
      [ROOT, "kuzz", "activity"],
      ["reply-1", "kuzz", "activity"],
    ],
  );
  const unread = inboxSummaryUnread([dm, { ...dm, key: "t", type: "thread" }], {
    [QUIET]: 50,
  });
  assert.deepEqual(unread.get(`dm:${QUIET}`), {
    count: 4,
    latestAt: 100,
    sentReadAt: 50,
  });
});

test("later pages append without duplicating rows", () => {
  const merged = mergeInboxSummaryPage(state([room(ROOM, 300)]), {
    next: null,
    rows: [room(ROOM, 300), room(QUIET, 200)],
  });
  assert.deepEqual(
    merged.rows.map((row) => row.key),
    [ROOM, QUIET],
  );
});

test("server unread applies only while the sent marker is still current", () => {
  const row = (conversationId, unreadCount, id = conversationId) => ({
    conversationId,
    id,
    item: { channelId: ROOM },
    unreadCount,
  });
  const rows = [
    row(`channel:${ROOM}`, 0),
    row(ROOT, 1),
    row("moved", 0),
    row("settled", 0),
    row("other", 2),
  ];
  const server = (sentReadAt) => ({ count: 12, latestAt: 300, sentReadAt });
  const unread = new Map([
    [`channel:${ROOM}`, server(100)],
    [ROOT, server(300)],
    ["moved", server(100)],
    ["settled", server(100)],
  ]);
  const markers = {
    [ROOM]: 100,
    [`thread:${ROOT}`]: 300,
    "thread:moved": 200,
    "thread:settled": 100,
  };
  const next = withServerUnreadCounts(
    rows,
    unread,
    (candidate) =>
      inboxRowReadAt(
        candidate,
        (id) => markers[id] ?? null,
        (root) => markers[`thread:${root}`] ?? null,
      ),
    (candidate) => candidate.conversationId === "settled",
  );
  assert.deepEqual(
    next.map((candidate) => candidate.unreadCount),
    // counted, read up to the latest, marker moved, read/muted, no server row
    [12, 1, 0, 0, 2],
  );
  assert.equal(next[4], rows[4]);
});

test("only deletions of shown messages touch the summary", () => {
  const current = state([thread(150), room(QUIET, 100)]);
  assert.deepEqual(
    deletedEventIds({
      tags: [
        ["e", "x"],
        ["k", "7"],
      ],
    }),
    ["x"],
  );
  assert.equal(inboxSummaryShowsAny(current, ["reaction-id"]), false);
  assert.equal(inboxSummaryShowsAny(current, [ROOT]), true);
  assert.equal(inboxSummaryShowsAny(current, ["reply-1"]), true);
  assert.equal(inboxSummaryShowsAny(current, []), false);
});

test("forum events stay out of the chat list", () => {
  const forum = {
    ...room(ROOM, 100),
    latest: { ...event("post", 100), kind: 45001 },
  };
  assert.deepEqual(inboxSummaryFeedItems([forum], new Map()), []);
  const current = state([room(ROOM, 100)]);
  assert.equal(
    applyLiveEventToInboxSummary(
      current,
      { ...event("comment", 200), kind: 45003 },
      channel,
      ME,
    ),
    current,
  );
});

test("a reply sent earlier than the relay receive time still updates the row", () => {
  // Relay `at` (receive time) is ahead of the reply's own created_at.
  const current = state([{ ...thread(150), at: 500, at_us: 500_000_000 }]);
  const next = applyLiveEventToInboxSummary(
    current,
    event("late-clock", 160, [["e", ROOT, "", "reply"]]),
    channel,
    ME,
  );
  assert.equal(next.rows[0].latest.id, "late-clock");
});
