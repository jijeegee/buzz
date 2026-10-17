import assert from "node:assert/strict";
import test from "node:test";

import { QueryClient, QueryObserver } from "@tanstack/react-query";

import { chatListQueryKey } from "./chatListState.ts";
import {
  CHAT_LIST_INVALIDATE_DEBOUNCE_MS,
  CHAT_LIST_MISSING_ROW_THROTTLE_MS,
  ChatListLiveSync,
  recordChatListPatches,
  replayChatListPatches,
} from "./chatListSync.ts";

const ME = "a".repeat(64);
const OTHER = "b".repeat(64);
const ROOM = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
const NEW_ROOM = "8ad375a7-6990-4b22-985f-e3fd34f634d7";
const KEY = chatListQueryKey("wss://relay", ME, []);

function fakeTimers() {
  let now = 0;
  let nextId = 1;
  const timers = new Map();
  return {
    host: {
      setTimeout(handler, ms) {
        const id = nextId++;
        timers.set(id, { at: now + ms, handler });
        return id;
      },
      clearTimeout(id) {
        timers.delete(id);
      },
    },
    now: () => now,
    advance(ms) {
      const until = now + ms;
      for (;;) {
        const due = [...timers]
          .filter(([, timer]) => timer.at <= until)
          .sort((a, b) => a[1].at - b[1].at)[0];
        if (!due) break;
        timers.delete(due[0]);
        now = due[1].at;
        due[1].handler();
      }
      now = until;
    },
  };
}

function roomRow(unread, at = 100) {
  return {
    type: "room",
    key: ROOM,
    at,
    at_us: at * 1_000_000,
    channel_id: ROOM,
    channel_type: "stream",
    channel_name: "general",
    archived: false,
    dm_members: [],
    root_id: null,
    thread_name: null,
    follow_state: null,
    closed: false,
    last_seq: 1,
    read_seq: 0,
    unread,
    unread_overflow: false,
    unread_mentions: 0,
    unread_mentions_overflow: false,
    unread_mark: false,
    latest: null,
    root: null,
  };
}

function message(id, createdAt, channelId = ROOM) {
  return {
    content: id,
    created_at: createdAt,
    id,
    kind: 9,
    pubkey: OTHER,
    tags: [["h", channelId]],
  };
}

function setup({ hidden = new Set() } = {}) {
  const queryClient = new QueryClient();
  const timers = fakeTimers();
  const invalidations = [];
  const original = queryClient.invalidateQueries.bind(queryClient);
  queryClient.invalidateQueries = (filters) => {
    if (filters?.type !== "inactive") invalidations.push(filters);
    return original({ ...filters, refetchType: "none" });
  };
  queryClient.setQueryData(KEY, {
    generation: 1,
    next: null,
    rows: [roomRow(0)],
  });
  const observer = new QueryObserver(queryClient, {
    queryFn: async () => queryClient.getQueryData(KEY),
    queryKey: KEY,
    staleTime: Number.POSITIVE_INFINITY,
  });
  const unsubscribe = observer.subscribe(() => {});
  const sync = new ChatListLiveSync(queryClient, {
    isRoomHidden: (channelId) => hidden.has(channelId),
    now: timers.now,
    timers: timers.host,
  });
  return {
    done: () => {
      unsubscribe();
      sync.dispose();
      queryClient.clear();
    },
    invalidations,
    queryClient,
    rows: () => queryClient.getQueryData(KEY).rows,
    sync,
    timers,
  };
}

test("a repeated live event is counted once", () => {
  const h = setup();
  try {
    h.sync.onMessage(message("m1", 200), ROOM, ME, false);
    h.sync.onMessage(message("m2", 200), ROOM, ME, false);
    h.sync.onMessage(message("m1", 200), ROOM, ME, false);
    assert.equal(h.rows()[0].unread, 2);
  } finally {
    h.done();
  }
});

test("patches made while a first page is fetched are re-applied to it", () => {
  const h = setup();
  try {
    const stop = recordChatListPatches(h.queryClient);
    h.sync.onMessage(message("during-fetch", 300), ROOM, ME, false);
    assert.equal(h.rows()[0].unread, 1);
    // The fetched page predates the event.
    const fetched = { generation: 2, next: null, rows: [roomRow(0, 100)] };
    const result = replayChatListPatches(fetched, stop());
    assert.equal(result.rows[0].unread, 1);
    assert.equal(result.rows[0].latest.id, "during-fetch");
    // A page that already has the event is left as it is.
    const fresh = {
      generation: 3,
      next: null,
      rows: [{ ...roomRow(1, 300), latest: message("during-fetch", 300) }],
    };
    const stop2 = recordChatListPatches(h.queryClient);
    h.sync.onMessage(message("during-fetch", 300), ROOM, ME, false);
    assert.equal(replayChatListPatches(fresh, stop2()), fresh);
  } finally {
    h.done();
  }
});

test("missing-row refetches are throttled; hidden or archived rooms never refetch", () => {
  const h = setup({ hidden: new Set(["hidden-dm"]) });
  try {
    h.sync.onMessage(message("n1", 200, NEW_ROOM), NEW_ROOM, ME, false);
    h.sync.refetchForMissingRow(NEW_ROOM);
    h.sync.refetchForMissingRow("hidden-dm");
    h.timers.advance(CHAT_LIST_INVALIDATE_DEBOUNCE_MS);
    assert.equal(h.invalidations.length, 1);
    h.sync.refetchForMissingRow(NEW_ROOM);
    h.sync.refetchForMissingRow(NEW_ROOM);
    h.timers.advance(CHAT_LIST_MISSING_ROW_THROTTLE_MS / 2);
    assert.equal(h.invalidations.length, 1);
    h.timers.advance(
      CHAT_LIST_MISSING_ROW_THROTTLE_MS / 2 + CHAT_LIST_INVALIDATE_DEBOUNCE_MS,
    );
    assert.equal(h.invalidations.length, 2);
    h.sync.refetchForMissingRow("hidden-dm");
    h.timers.advance(CHAT_LIST_MISSING_ROW_THROTTLE_MS * 2);
    assert.equal(h.invalidations.length, 2);
  } finally {
    h.done();
  }
});
