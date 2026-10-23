import assert from "node:assert/strict";
import test from "node:test";

const storage = new Map();
globalThis.localStorage = {
  getItem: (key) => (storage.has(key) ? storage.get(key) : null),
  setItem: (key, value) => storage.set(key, String(value)),
  removeItem: (key) => storage.delete(key),
  clear: () => storage.clear(),
  key: (index) => [...storage.keys()][index] ?? null,
  get length() {
    return storage.size;
  },
};
globalThis.window ??= globalThis;

const { QueryClient } = await import("@tanstack/react-query");
const { createOptimisticMessage, restoreStoredSends } = await import(
  "../hooks.ts"
);
const { emptyChannelWindowStore, replaceNewestChannelWindow } = await import(
  "./channelWindowStore.ts"
);
const { discardFailedSend, hasFailedSend, resetFailedSends, retryFailedSend } =
  await import("./failedSends.ts");
const {
  earlierStoredSends,
  removeStoredSend,
  resetStoredSendSession,
  saveStoredSend,
} = await import("./failedSendStore.ts");
const { channelMessagesKey, channelWindowKey, threadRepliesKey } = await import(
  "./messageQueryKeys.ts"
);
const { initDraftStore } = await import("./useDrafts.ts");

const CHANNEL = "channel";
const ME = "a".repeat(64);
const OTHER = "c".repeat(64);
const RELAY = "wss://relay.example";

function relayEvent(id, createdAt, content, extraTags = []) {
  return {
    id: id.padEnd(64, "0"),
    pubkey: ME,
    created_at: createdAt,
    kind: 9,
    tags: [["h", CHANNEL], ...extraTags],
    content,
    sig: "b".repeat(128),
  };
}

function client(rows = []) {
  // No garbage-collection timers: with `window` defined they would keep the
  // test process alive.
  const queryClient = new QueryClient({
    defaultOptions: { queries: { gcTime: Number.POSITIVE_INFINITY } },
  });
  queryClient.setQueryData(
    channelWindowKey(CHANNEL),
    replaceNewestChannelWindow(emptyChannelWindowStore(), {
      startCursor: null,
      rows: rows.map((event) => ({ event, thread: null })),
      aux: [],
      nextCursor: null,
      hasMore: false,
    }),
  );
  queryClient.setQueryData(channelMessagesKey(CHANNEL), rows);
  return queryClient;
}

function store(localKey, content, parentEventId = null, rootId = null) {
  const event = createOptimisticMessage(
    CHANNEL,
    content,
    { pubkey: ME },
    [],
    [],
    null,
    [],
    null,
    null,
    localKey,
  );
  saveStoredSend({
    localKey,
    channelId: CHANNEL,
    rootId,
    event,
    send: {
      content,
      mentionPubkeys: [],
      mediaTags: [],
      parentEventId,
      sentFromThreadRootId: null,
      sentFromThreadRootExcerpt: null,
      forceRest: false,
    },
  });
  return event;
}

/** A fresh app start: same account on disk, nothing in memory. */
function restart() {
  resetFailedSends();
  resetStoredSendSession();
}

function rows(queryClient) {
  return queryClient.getQueryData(channelMessagesKey(CHANNEL));
}

test.beforeEach(() => {
  storage.clear();
  initDraftStore(ME, RELAY);
  restart();
});

test("a send from this session is not restored as failed while in flight", () => {
  store("local-1", "hi");
  assert.deepEqual(earlierStoredSends(CHANNEL), []);
});

test("an unsent message survives a restart as a failed row", () => {
  store("local-1", "hi");
  restart();
  const queryClient = client();
  restoreStoredSends(queryClient, CHANNEL, async () => {}, new Set());
  const row = rows(queryClient).find((event) => event.id === "local-1");
  assert.equal(row?.content, "hi");
  assert.equal(row?.failed, true);
  assert.equal(hasFailedSend("local-1"), true);
});

test("Retry after a restart sends the stored message again under its row", () => {
  store("local-1", "hi");
  restart();
  const queryClient = client();
  const calls = [];
  restoreStoredSends(
    queryClient,
    CHANNEL,
    async (variables) => calls.push(variables),
    new Set(),
  );
  retryFailedSend("local-1");
  assert.equal(calls.length, 1);
  assert.equal(calls[0].content, "hi");
  assert.equal(calls[0].channelId, CHANNEL);
  assert.equal(calls[0].optimisticLocalKey, "local-1");
});

test("Delete after a restart removes the row and the stored copy", () => {
  store("local-1", "hi");
  restart();
  const queryClient = client();
  restoreStoredSends(queryClient, CHANNEL, async () => {}, new Set());
  discardFailedSend("local-1");
  assert.equal(
    rows(queryClient).some((event) => event.id === "local-1"),
    false,
  );
  restart();
  assert.deepEqual(earlierStoredSends(CHANNEL), []);
});

test("a message the relay did store is not shown twice after a restart", () => {
  const preview = store("local-1", "hi");
  restart();
  const queryClient = client([relayEvent("echo", preview.created_at, "hi")]);
  restoreStoredSends(queryClient, CHANNEL, async () => {}, new Set());
  assert.equal(
    rows(queryClient).filter((event) => event.content === "hi").length,
    1,
  );
  assert.equal(hasFailedSend("local-1"), false);
  assert.deepEqual(earlierStoredSends(CHANNEL), []);
});

test("a delivered send leaves nothing to restore", () => {
  store("local-1", "hi");
  removeStoredSend("local-1");
  restart();
  assert.deepEqual(earlierStoredSends(CHANNEL), []);
});

test("another account on this device does not see the stored message", () => {
  store("local-1", "hi");
  initDraftStore(OTHER, RELAY);
  restart();
  assert.deepEqual(earlierStoredSends(CHANNEL), []);
  initDraftStore(ME, RELAY);
  assert.equal(earlierStoredSends(CHANNEL).length, 1);
});

test("a failed thread reply returns to its open thread", () => {
  const root = relayEvent("root", 100, "root");
  store("local-2", "reply", root.id, root.id);
  restart();
  const queryClient = client([root]);
  queryClient.setQueryData(threadRepliesKey(CHANNEL, root.id), []);
  restoreStoredSends(queryClient, CHANNEL, async () => {}, new Set());
  const replies = queryClient.getQueryData(threadRepliesKey(CHANNEL, root.id));
  assert.deepEqual(
    replies.map((event) => event.id),
    ["local-2"],
  );
  assert.equal(replies[0].failed, true);
});

test("restoring twice does not draw the row twice", () => {
  store("local-1", "hi");
  restart();
  const queryClient = client();
  const attempted = new Set();
  restoreStoredSends(queryClient, CHANNEL, async () => {}, attempted);
  restoreStoredSends(queryClient, CHANNEL, async () => {}, attempted);
  assert.equal(
    rows(queryClient).filter((event) => event.id === "local-1").length,
    1,
  );
});
