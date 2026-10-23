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
  removeStoredSend,
  resetStoredSendSession,
  saveStoredSend,
  settleStoredSend,
  storedSends,
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

function client(rows = [], hasMore = false) {
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
      nextCursor: hasMore
        ? { created_at: rows[0].created_at, event_id: rows[0].id }
        : null,
      hasMore,
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

test("a send still in flight is not drawn as failed", () => {
  store("local-1", "hi");
  const queryClient = client();
  restoreStoredSends(queryClient, CHANNEL, async () => {}, new Set());
  assert.equal(
    rows(queryClient).some((event) => event.id === "local-1"),
    false,
  );
  settleStoredSend("local-1");
  restoreStoredSends(queryClient, CHANNEL, async () => {}, new Set());
  assert.equal(
    rows(queryClient).find((event) => event.id === "local-1")?.failed,
    true,
  );
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
  assert.deepEqual(storedSends(CHANNEL), []);
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
  assert.deepEqual(storedSends(CHANNEL), []);
});

test("a delivered send leaves nothing to restore", () => {
  store("local-1", "hi");
  removeStoredSend("local-1");
  restart();
  assert.deepEqual(storedSends(CHANNEL), []);
});

test("another account on this device does not see the stored message", () => {
  store("local-1", "hi");
  initDraftStore(OTHER, RELAY);
  restart();
  assert.deepEqual(storedSends(CHANNEL), []);
  initDraftStore(ME, RELAY);
  assert.equal(storedSends(CHANNEL).length, 1);
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

function relayCheck(lookup) {
  let settled = 0;
  return {
    check: {
      state: new Map(),
      lookup,
      onSettled: () => {
        settled += 1;
      },
      isDisposed: () => false,
    },
    settled: () => settled,
  };
}

test("a send the relay stored before the app quit is not shown as failed", async () => {
  const preview = store("local-1", "hi");
  restart();
  const queryClient = client();
  const { check } = relayCheck(async () => [
    relayEvent("landed", preview.created_at + 1, "hi"),
  ]);
  restoreStoredSends(queryClient, CHANNEL, async () => {}, new Set(), check);
  assert.equal(
    rows(queryClient).some((event) => event.id === "local-1"),
    false,
  );
  await new Promise((resolve) => setImmediate(resolve));
  restoreStoredSends(queryClient, CHANNEL, async () => {}, new Set(), check);
  assert.equal(
    rows(queryClient).some((event) => event.id === "local-1"),
    false,
  );
  assert.equal(hasFailedSend("local-1"), false);
  assert.deepEqual(storedSends(CHANNEL), []);
});

test("an unreachable relay still shows the send as failed", async () => {
  store("local-1", "hi");
  restart();
  const queryClient = client();
  const { check, settled } = relayCheck(async () => {
    throw new Error("offline");
  });
  restoreStoredSends(queryClient, CHANNEL, async () => {}, new Set(), check);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(settled(), 1);
  restoreStoredSends(queryClient, CHANNEL, async () => {}, new Set(), check);
  assert.equal(
    rows(queryClient).find((event) => event.id === "local-1")?.failed,
    true,
  );
});

test("a late echo adopting a failed row clears its stored copy", () => {
  const preview = store("local-1", "hi");
  settleStoredSend("local-1");
  const echo = { ...relayEvent("echo", preview.created_at, "hi") };
  echo.localKey = "local-1";
  const queryClient = client([echo]);
  restoreStoredSends(queryClient, CHANNEL, async () => {}, new Set());
  assert.deepEqual(storedSends(CHANNEL), []);
});

test("the same text sent much later does not count as delivered", () => {
  const preview = store("local-1", "ok");
  restart();
  const queryClient = client([
    relayEvent("later", preview.created_at + 3600, "ok"),
  ]);
  restoreStoredSends(queryClient, CHANNEL, async () => {}, new Set());
  assert.equal(storedSends(CHANNEL).length, 1);
});

test("Retry marks the send in flight before it starts", () => {
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
  // A cache update before the mutation saves must not arm Retry again.
  restoreStoredSends(queryClient, CHANNEL, async () => {}, new Set());
  assert.equal(hasFailedSend("local-1"), false);
  assert.equal(calls.length, 1);
});

test("unsent messages older than a week are let go", () => {
  const preview = store("local-1", "hi");
  const key = [...storage.keys()].find((k) =>
    k.startsWith("kuzz-failed-sends.v1:"),
  );
  const entries = JSON.parse(storage.get(key));
  entries[0].event.created_at = preview.created_at - 8 * 24 * 60 * 60;
  storage.set(key, JSON.stringify(entries));
  restart();
  assert.deepEqual(storedSends(CHANNEL), []);
});

test("at most 50 unsent messages are kept", () => {
  for (let i = 0; i < 55; i += 1) store(`local-${i}`, `m${i}`);
  const kept = storedSends(CHANNEL).map((stored) => stored.localKey);
  assert.equal(kept.length, 50);
  assert.equal(kept[0], "local-5");
});

test("a send settled after an account switch clears its own account's copy", () => {
  store("local-1", "hi");
  initDraftStore(OTHER, RELAY);
  removeStoredSend("local-1");
  initDraftStore(ME, RELAY);
  assert.deepEqual(storedSends(CHANNEL), []);
});

test("a failed send older than the loaded messages is drawn at the bottom", () => {
  const preview = store("local-1", "hi");
  const key = [...storage.keys()].find((k) =>
    k.startsWith("kuzz-failed-sends.v1:"),
  );
  const entries = JSON.parse(storage.get(key));
  entries[0].event.created_at = preview.created_at - 600;
  storage.set(key, JSON.stringify(entries));
  restart();
  const newer = relayEvent("newer", preview.created_at - 300, "newer");
  const queryClient = client([newer], true);
  restoreStoredSends(queryClient, CHANNEL, async () => {}, new Set());
  const row = rows(queryClient).find((event) => event.id === "local-1");
  assert.equal(row?.failed, true);
  assert.equal(row?.content, "hi");
});

test("an earlier message with the same text does not count as delivered", () => {
  const preview = store("local-1", "ok");
  restart();
  const queryClient = client([
    relayEvent("earlier", preview.created_at - 30, "ok"),
  ]);
  restoreStoredSends(queryClient, CHANNEL, async () => {}, new Set());
  assert.equal(storedSends(CHANNEL).length, 1);
  assert.equal(hasFailedSend("local-1"), true);
});

test("a relay copy arriving later takes away the failed row and its Retry", () => {
  const preview = store("local-1", "hi");
  restart();
  const queryClient = client();
  restoreStoredSends(queryClient, CHANNEL, async () => {}, new Set());
  assert.equal(hasFailedSend("local-1"), true);
  // A refetch brings the relay's copy without adopting the failed row.
  queryClient.setQueryData(channelMessagesKey(CHANNEL), (current) => [
    ...current,
    relayEvent("landed", preview.created_at, "hi"),
  ]);
  restoreStoredSends(queryClient, CHANNEL, async () => {}, new Set());
  assert.equal(
    rows(queryClient).some((event) => event.id === "local-1"),
    false,
  );
  assert.equal(hasFailedSend("local-1"), false);
  assert.deepEqual(storedSends(CHANNEL), []);
});
