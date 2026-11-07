// R1 for scroll-up: an older room or thread window the store fully holds is
// drawn at once; the check's answer (or the refetch after it) then replaces
// what was drawn.
import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

// The Tauri bridge reads window.__TAURI_INTERNALS__.
globalThis.window ??= globalThis;

/** What the relay answers a page with, per command. */
const relay = { room: [], thread: null };
globalThis.__TAURI_INTERNALS__ = {
  invoke: (command) => {
    if (command === "get_channel_window") return Promise.resolve(relay.room);
    if (command === "get_thread_windows") {
      return Promise.resolve(relay.thread ? [relay.thread] : []);
    }
    return Promise.reject(new Error(`unmocked Tauri command: ${command}`));
  },
  transformCallback: () => Math.random(),
};

const { QueryClient } = await import("@tanstack/react-query");
const { setStoreScope } = await import("./paintCoverage.ts");
const { channelWindowKey, threadRepliesKey } = await import(
  "./messageQueryKeys.ts"
);
const { pageOlderMessagesUntilRowFloor } = await import(
  "./pageOlderMessages.ts"
);
const { loadOlderThreadReplies, threadWindowStateKey } = await import(
  "./threadWindowCache.ts"
);
const { resetWindowCheckGuards, setWindowCheckDeps } = await import(
  "./windowCheck.ts"
);
const { noteServerWindowCheckMode, resetWindowCheckModes } = await import(
  "./windowCheckMode.ts"
);

const RELAY = "wss://relay.test";
const ROOM = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
const ROOT = "d".repeat(64);
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

function event(n, tags = [["h", ROOM]]) {
  return {
    id: n.toString(16).padStart(64, "0"),
    kind: 9,
    created_at: n,
    pubkey: "b".repeat(64),
    content: `m${n}`,
    sig: "",
    tags,
  };
}
const reply = (n) =>
  event(n, [
    ["h", ROOM],
    ["e", ROOT, "", "root"],
  ]);
const bounds = {
  id: "f".repeat(64),
  kind: 39006,
  created_at: 400,
  pubkey: "c".repeat(64),
  content: JSON.stringify({ has_more: false, next_cursor: null }),
  sig: "",
  // The window below row 300, as the relay and the store key it.
  tags: [
    ["h", ROOM],
    ["d", `${ROOM}:300:${(300).toString(16).padStart(64, "0")}`],
  ],
};

const clients = [];
function client() {
  const queryClient = new QueryClient();
  clients.push(queryClient);
  setStoreScope(queryClient, { pubkey: "me", relayUrl: RELAY });
  return queryClient;
}

/** The store holds `stored`; the check answers when `answer()` is called. */
function relayOn(stored) {
  noteServerWindowCheckMode(RELAY, "on", Date.now());
  const pending = {};
  const restore = setWindowCheckDeps({
    stored: async () => stored,
    check: () =>
      new Promise((resolve) => {
        pending.answer = (check) =>
          resolve({ answer: check, noCheck: false, page: null });
      }),
    mark: async () => "mark",
    release: async () => {},
    replace: async () => true,
  });
  return { pending, restore };
}

const restores = [];
afterEach(() => {
  for (const restore of restores.splice(0)) restore();
  for (const queryClient of clients.splice(0)) queryClient.clear();
  resetWindowCheckModes();
  resetWindowCheckGuards();
  relay.room = [];
  relay.thread = null;
});

const rowIds = (store) =>
  store.pages.map((page) => page.rows.map((row) => row.event.created_at));

test("scrolling up draws the stored room window, then the refetch replaces it", async () => {
  const queryClient = client();
  const newest = event(300);
  queryClient.setQueryData(channelWindowKey(ROOM), {
    pages: [
      {
        startCursor: null,
        rows: [{ event: newest, thread: null }],
        aux: [],
        nextCursor: { createdAt: 300, eventId: newest.id },
        hasMore: true,
      },
    ],
    liveOverlay: [],
    liveAux: [],
    liveSummaries: {},
  });
  const fake = relayOn({
    events: [event(200), event(100), bounds],
    count: 2,
    oldest: { createdAt: 100, eventId: event(100).id },
    hasMore: false,
    fullyStored: true,
  });
  restores.push(fake.restore);
  // The relay no longer has row 100.
  relay.room = [event(200), bounds];

  const paging = pageOlderMessagesUntilRowFloor(
    queryClient,
    ROOM,
    () => true,
    2,
  );
  await tick();
  assert.deepEqual(
    rowIds(queryClient.getQueryData(channelWindowKey(ROOM))),
    [[300], [200, 100]],
    "drawn while the check is out",
  );
  fake.pending.answer({ oldestId: event(200).id, count: 1, mode: "on" });
  await paging;
  assert.deepEqual(rowIds(queryClient.getQueryData(channelWindowKey(ROOM))), [
    [300],
    [200],
  ]);
});

test("scrolling up draws the stored thread replies, then the refetch replaces them", async () => {
  const queryClient = client();
  queryClient.setQueryData(threadRepliesKey(ROOM, ROOT), [reply(300)]);
  const olderCursor = { createdAt: 300, eventId: reply(300).id };
  queryClient.setQueryData(threadWindowStateKey(ROOM, ROOT), {
    hasOlder: true,
    olderCursor,
    newestCovered: olderCursor,
  });
  const fake = relayOn({
    events: [reply(200), reply(100)],
    count: 2,
    oldest: { createdAt: 100, eventId: reply(100).id },
    hasMore: false,
    fullyStored: true,
  });
  restores.push(fake.restore);
  relay.thread = {
    channel_id: ROOM,
    root_id: ROOT,
    events: [reply(200)],
    has_more: false,
    next_cursor: null,
  };

  const loading = loadOlderThreadReplies(queryClient, ROOM, ROOT, 2);
  await tick();
  const created = () =>
    queryClient
      .getQueryData(threadRepliesKey(ROOM, ROOT))
      .map((reply) => reply.created_at);
  assert.deepEqual(created(), [100, 200, 300], "drawn while the check is out");
  fake.pending.answer({ oldestId: reply(200).id, count: 1, mode: "on" });
  assert.equal(await loading, true);
  assert.deepEqual(created(), [200, 300]);
  assert.equal(
    queryClient.getQueryData(threadWindowStateKey(ROOM, ROOT)).hasOlder,
    false,
  );
});
