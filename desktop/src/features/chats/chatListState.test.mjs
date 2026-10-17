import assert from "node:assert/strict";
import test from "node:test";

import {
  applyLiveEventToChatList,
  chatListQueryKey,
  chatRowContextId,
  chatRowUnread,
  chatThreadTitle,
  formatChatCount,
  mergeChatListPage,
  orderChatRows,
} from "./chatListState.ts";

const ME = "a".repeat(64);
const OTHER = "b".repeat(64);
const ROOM = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
const DM = "8ad375a7-6990-4b22-985f-e3fd34f634d7";
const ROOT = "c".repeat(64);
const OTHER_ROOT = "d".repeat(64);

function event(id, createdAt, tags = [], pubkey = OTHER, kind = 9) {
  return {
    content: `message ${id}`,
    created_at: createdAt,
    id,
    kind,
    pubkey,
    tags: [["h", ROOM], ...tags],
  };
}

const reply = (root) => [["e", root, "", "reply"]];

function row(overrides = {}) {
  const type = overrides.type ?? "room";
  const at = overrides.at ?? 100;
  return {
    type,
    key:
      type === "thread"
        ? (overrides.root_id ?? ROOT)
        : (overrides.channel_id ?? ROOM),
    at,
    at_us: at * 1_000_000,
    channel_id: ROOM,
    channel_type: "stream",
    channel_name: "general",
    archived: false,
    dm_members: [],
    root_id: type === "thread" ? ROOT : null,
    thread_name: null,
    follow_state: type === "thread" ? "auto" : null,
    closed: false,
    last_seq: 10,
    read_seq: 8,
    unread: 2,
    unread_overflow: false,
    unread_mentions: 0,
    unread_mentions_overflow: false,
    unread_mark: false,
    latest: event(`latest-${type}`, at),
    root: null,
    ...overrides,
  };
}

function state(rows, next = null) {
  return { generation: 1, next, rows };
}

test("later pages append without duplicating rows", () => {
  const first = state([row(), row({ type: "thread", at: 90 })], {
    at_us: 90_000_000,
    key: ROOT,
  });
  const merged = mergeChatListPage(first, {
    next: null,
    rows: [
      row({ type: "thread", at: 90 }),
      row({ channel_id: DM, key: DM, at: 80 }),
    ],
  });
  assert.deepEqual(
    merged.rows.map((r) => r.key),
    [ROOM, ROOT, DM],
  );
  assert.equal(merged.next, null);
  assert.equal(merged.generation, 1);
});

test("others' top-level messages move the room row and count unread and mentions", () => {
  const current = state([row({ channel_id: DM, key: DM, at: 150 }), row()]);
  const plain = applyLiveEventToChatList(
    current,
    event("n1", 200),
    ROOM,
    ME,
    false,
  );
  assert.equal(plain.refetch, false);
  assert.deepEqual(
    plain.state.rows.map((r) => r.key),
    [ROOM, DM],
  );
  assert.equal(plain.state.rows[0].unread, 3);
  assert.equal(plain.state.rows[0].unread_mentions, 0);
  assert.equal(plain.state.rows[0].latest.id, "n1");

  const mention = applyLiveEventToChatList(
    plain.state,
    event("n2", 201),
    ROOM,
    ME,
    true,
  );
  assert.equal(mention.state.rows[0].unread, 4);
  assert.equal(mention.state.rows[0].unread_mentions, 1);

  // Replays and older events change nothing.
  assert.equal(
    applyLiveEventToChatList(mention.state, event("n2", 201), ROOM, ME, true)
      .state,
    mention.state,
  );
  assert.equal(
    applyLiveEventToChatList(mention.state, event("old", 10), ROOM, ME, false)
      .state,
    mention.state,
  );
  // Reactions and other kinds are not messages.
  assert.equal(
    applyLiveEventToChatList(
      mention.state,
      event("r", 300, [], OTHER, 7),
      ROOM,
      ME,
      false,
    ).state,
    mention.state,
  );
});

test("counts stop at the relay cap and flag overflow", () => {
  const current = state([row({ unread: 99, unread_overflow: true })]);
  const next = applyLiveEventToChatList(
    current,
    event("n", 200),
    ROOM,
    ME,
    false,
  );
  assert.equal(next.state.rows[0].unread, 99);
  assert.equal(next.state.rows[0].unread_overflow, true);
  assert.equal(formatChatCount(99, true), "99+");
  assert.equal(formatChatCount(5, false), "5");
});

test("my own message marks the row read and clears an unread mark", () => {
  const current = state([
    row({ unread: 4, unread_mentions: 2, unread_mark: true }),
  ]);
  const next = applyLiveEventToChatList(
    current,
    event("mine", 200, [], ME),
    ROOM,
    ME,
    false,
  );
  const [room] = next.state.rows;
  assert.equal(room.unread, 0);
  assert.equal(room.unread_mentions, 0);
  assert.equal(room.unread_mark, false);
});

test("a reply moves only its thread row; a broadcast reply also moves the room", () => {
  const current = state([
    row({ at: 150 }),
    row({ type: "thread", at: 100, unread: 0 }),
  ]);
  const threadOnly = applyLiveEventToChatList(
    current,
    event("r1", 200, reply(ROOT)),
    ROOM,
    ME,
    false,
  );
  const byKey = (s) => new Map(s.rows.map((r) => [r.key, r]));
  assert.deepEqual(
    threadOnly.state.rows.map((r) => r.key),
    [ROOT, ROOM],
  );
  assert.equal(byKey(threadOnly.state).get(ROOT).unread, 1);
  assert.equal(byKey(threadOnly.state).get(ROOM).unread, 2);

  const broadcast = applyLiveEventToChatList(
    current,
    event("r2", 200, [...reply(ROOT), ["broadcast", "1"]]),
    ROOM,
    ME,
    false,
  );
  assert.equal(byKey(broadcast.state).get(ROOT).unread, 1);
  assert.equal(byKey(broadcast.state).get(ROOM).unread, 3);
});

test("rows only the relay can place ask for a refetch", () => {
  const current = state([row()]);
  // A room not in the list (new DM, or beyond the loaded pages).
  assert.equal(
    applyLiveEventToChatList(current, event("x", 200), DM, ME, false).refetch,
    true,
  );
  // An unfollowed thread stays out unless I reply or am mentioned.
  const quiet = applyLiveEventToChatList(
    current,
    event("x", 200, reply(OTHER_ROOT)),
    ROOM,
    ME,
    false,
  );
  assert.equal(quiet.refetch, false);
  assert.equal(quiet.state, current);
  assert.equal(
    applyLiveEventToChatList(
      current,
      event("x", 200, reply(OTHER_ROOT)),
      ROOM,
      ME,
      true,
    ).refetch,
    true,
  );
  assert.equal(
    applyLiveEventToChatList(
      current,
      event("x", 200, reply(OTHER_ROOT), ME),
      ROOM,
      ME,
      false,
    ).refetch,
    true,
  );
  // Close / reopen markers change which threads are listed.
  const close = applyLiveEventToChatList(
    state([row({ type: "thread" })]),
    event("c", 200, [...reply(ROOT), ["buzz:thread-closed", ROOT]]),
    ROOM,
    ME,
    false,
  );
  assert.equal(close.refetch, true);
  assert.equal(close.state.rows[0].unread, 2);
});

test("this device's newer read position hides server counts until the relay catches up", () => {
  const room = row({ unread: 3, unread_mentions: 1 });
  assert.deepEqual(chatRowUnread(room, null), {
    count: 3,
    overflow: false,
    mentions: 1,
    mentionsOverflow: false,
    marked: false,
  });
  assert.equal(chatRowUnread(room, 99).count, 3);
  assert.equal(chatRowUnread(room, 100).count, 0);
  assert.equal(chatRowUnread(room, 100).mentions, 0);
  // Marked unread wins over a local read position.
  const marked = row({ unread: 0, unread_mark: true });
  assert.equal(chatRowUnread(marked, 500).marked, true);
  assert.equal(chatRowContextId(room), ROOM);
  assert.equal(chatRowContextId(row({ type: "thread" })), `thread:${ROOT}`);
});

test("pinned rooms and threads come first, each group by activity", () => {
  const rows = [
    row({ channel_id: DM, key: DM, at: 300 }),
    row({ type: "thread", at: 200 }),
    row({ at: 100 }),
    row({ type: "thread", root_id: OTHER_ROOT, key: OTHER_ROOT, at: 50 }),
  ];
  assert.deepEqual(
    orderChatRows(rows, new Set([ROOM]), new Set([OTHER_ROOT])).map(
      (r) => r.key,
    ),
    [ROOM, OTHER_ROOT, DM, ROOT],
  );
  assert.deepEqual(
    orderChatRows(rows, undefined, undefined).map((r) => r.key),
    [DM, ROOT, ROOM, OTHER_ROOT],
  );
});

test("thread titles fall back to the root's first line", () => {
  assert.equal(
    chatThreadTitle(row({ type: "thread", thread_name: " Launch " })),
    "Launch",
  );
  assert.equal(
    chatThreadTitle(
      row({
        type: "thread",
        root: { ...event(ROOT, 1), content: "\n  First line \nmore" },
      }),
    ),
    "First line",
  );
  assert.equal(chatThreadTitle(row({ type: "thread" })), "Thread");
  assert.deepEqual(chatListQueryKey("wss://r", "ABC", ["x", "y"]), [
    "chat-list",
    "wss://r",
    "abc",
    "x,y",
  ]);
});
