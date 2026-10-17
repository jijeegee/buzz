import assert from "node:assert/strict";
import test from "node:test";

import {
  applyFollowResponseToChatList,
  applyLiveEventToChatList,
  applyMarkReadResponseToChatList,
  applyUnreadMarkToChatList,
  chatListQueryKey,
  isSameChatListAccount,
  createEventIdLru,
  mergeRefetchedFirstPage,
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
    event("c", 200, [...reply(ROOT), ["kuzz:thread-closed", ROOT]]),
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

test("own mark_read responses patch rows: caught up clears counts, marks show", () => {
  const room = row({ last_seq: 10, read_seq: 8, unread: 2 });
  const thread = row({ type: "thread", last_seq: 12, read_seq: 3, unread: 4 });
  const current = state([room, thread]);
  const patched = applyMarkReadResponseToChatList(current, {
    items: [
      {
        channel: ROOM,
        channel_id: ROOM,
        read_seq: 10,
        stored: true,
        unread_mark: false,
      },
      {
        channel_id: ROOM,
        read_seq: 3,
        stored: true,
        thread: ROOT,
        unread_mark: true,
      },
      // Not stored (no follow row): nothing to patch.
      {
        channel_id: ROOM,
        read_seq: null,
        stored: false,
        thread: OTHER_ROOT,
        unread_mark: false,
      },
    ],
    mark_read: 1,
  });
  assert.deepEqual(
    patched.rows.map((r) => [r.type, r.read_seq, r.unread, r.unread_mark]),
    [
      ["room", 10, 0, false],
      ["thread", 3, 4, true],
    ],
  );
  assert.equal(applyMarkReadResponseToChatList(current, null), current);
  assert.equal(
    applyMarkReadResponseToChatList(current, { items: [] }).rows,
    current.rows,
  );
});

test("own follow responses drop unfollowed threads; unknown follows need the relay", () => {
  const current = state([row(), row({ type: "thread" })]);
  const unfollowed = applyFollowResponseToChatList(current, {
    root: ROOT,
    state: "unfollowed",
    thread_follow: 1,
  });
  assert.deepEqual(
    unfollowed.state.rows.map((r) => r.type),
    ["room"],
  );
  assert.equal(unfollowed.refetch, false);
  const followed = applyFollowResponseToChatList(current, {
    root: ROOT,
    state: "followed",
  });
  assert.equal(followed.state.rows[1].follow_state, "followed");
  assert.equal(followed.refetch, false);
  assert.equal(
    applyFollowResponseToChatList(current, {
      root: OTHER_ROOT,
      state: "followed",
    }).refetch,
    true,
  );
});

test("a refetched first page keeps the older loaded rows and their cursor", () => {
  const older = row({ channel_id: DM, at: 10 });
  const previous = state(
    [row({ at: 100 }), row({ type: "thread", at: 50 }), older],
    {
      at_us: 10_000_000,
      key: DM,
    },
  );
  const merged = mergeRefetchedFirstPage(
    previous,
    {
      next: { at_us: 50_000_000, key: ROOT },
      rows: [row({ type: "thread", at: 200 }), row({ at: 100 })],
    },
    7,
  );
  assert.equal(merged.generation, 7);
  assert.deepEqual(merged.next, previous.next);
  assert.deepEqual(
    merged.rows.map((r) => r.key),
    [ROOT, ROOM, DM],
  );
  // A complete first page replaces everything.
  const complete = mergeRefetchedFirstPage(
    previous,
    { next: null, rows: [row()] },
    8,
  );
  assert.deepEqual(
    complete.rows.map((r) => r.key),
    [ROOM],
  );
  assert.equal(complete.next, null);
});

test("replayed events apply only when newer than the fetched row", () => {
  const current = state([row({ at: 100, unread: 2 })]);
  const same = applyLiveEventToChatList(
    current,
    event("other-same-second", 100),
    ROOM,
    ME,
    false,
    true,
  );
  assert.equal(same.state, current);
  const newer = applyLiveEventToChatList(
    current,
    event("newer", 101),
    ROOM,
    ME,
    false,
    true,
  );
  assert.equal(newer.state.rows[0].unread, 3);
});

test("the event id LRU reports repeats and forgets the oldest", () => {
  const lru = createEventIdLru(2);
  assert.equal(lru.seen("a"), false);
  assert.equal(lru.seen("a"), true);
  assert.equal(lru.seen("b"), false);
  assert.equal(lru.seen("c"), false);
  assert.equal(lru.seen("a"), false);
  assert.equal(lru.seen("c"), true);
});

test("Mark as unread shows on the thread or room row right away", () => {
  const rows = [row(), row({ type: "thread" })];
  const thread = applyUnreadMarkToChatList(state(rows), { thread: ROOT });
  assert.equal(thread.rows[0].unread_mark, false);
  assert.equal(thread.rows[1].unread_mark, true);
  const room = applyUnreadMarkToChatList(state(rows), { channel: ROOM });
  assert.equal(room.rows[0].unread_mark, true);
  assert.equal(room.rows[1].unread_mark, false);
  const unchanged = state(rows);
  assert.equal(
    applyUnreadMarkToChatList(unchanged, { thread: OTHER_ROOT }),
    unchanged,
  );
});

test("a pin change keeps the same chat list account, an account switch does not", () => {
  const base = chatListQueryKey("wss://relay.example", "AB", []);
  assert.equal(
    isSameChatListAccount(
      base,
      chatListQueryKey("wss://relay.example", "ab", ["root1"]),
    ),
    true,
  );
  assert.equal(
    isSameChatListAccount(
      base,
      chatListQueryKey("wss://relay.example", "cd", []),
    ),
    false,
  );
  assert.equal(
    isSameChatListAccount(
      base,
      chatListQueryKey("wss://other.example", "ab", []),
    ),
    false,
  );
});
