import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";

// The Tauri bridge reads window.__TAURI_INTERNALS__.
globalThis.window ??= globalThis;

const ROOM = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
const ROOT = "d".repeat(64);
const SCOPE = { pubkey: "f".repeat(64), relayUrl: "wss://relay" };
const iso = (seconds) => new Date(seconds * 1000).toISOString();

/** Reply `n` is created at second `n`; a higher n is newer. */
function reply(n) {
  return {
    content: `r${n}`,
    created_at: n,
    id: n.toString(16).padStart(64, "0"),
    kind: 9,
    pubkey: "b".repeat(64),
    sig: "",
    tags: [
      ["h", ROOM],
      ["e", ROOT, "", "root"],
    ],
  };
}

const cursorOf = (n) => ({ createdAt: n, eventId: reply(n).id });

/** A store page holding replies `to`..`from`, newest first. */
function storePage(from, to, extra = {}) {
  const rows = [];
  for (let n = to; n >= from; n -= 1) rows.push(reply(n));
  return {
    rows,
    aux: [],
    summaries: [],
    nextCursor: null,
    hasMore: false,
    gap: false,
    coveredThrough: 100,
    liveCovered: false,
    ...extra,
  };
}

/** A relay page holding replies `from`..`to`, newest first. */
function relayPage(from, to, hasMore) {
  const events = [];
  for (let n = to; n >= from; n -= 1) events.push(reply(n));
  return {
    channel_id: ROOM,
    root_id: ROOT,
    events,
    has_more: hasMore,
    next_cursor: hasMore
      ? { created_at: from, event_id: reply(from).id }
      : null,
  };
}

let calls = [];
/** @type {(args: any) => any} */
let readStore = () => storePage(1, 0);
/** @type {(args: any) => any} */
let readRelay = () => relayPage(1, 3, false);

globalThis.__TAURI_INTERNALS__ = {
  invoke: async (command, args) => {
    calls.push({ command, args });
    switch (command) {
      case "message_store_load_heads":
        return [];
      case "message_store_read_thread_head":
        return readStore(args);
      case "get_thread_windows":
        return [readRelay(args)];
      case "get_events":
        return [];
      default:
        throw new Error(`unmocked Tauri command: ${command}`);
    }
  },
  transformCallback: () => Math.random(),
};

const { QueryClient } = await import("@tanstack/react-query");
const { hydrateChannelHeads } = await import("./storeChannelHeads.ts");
const { isProvenThreadHead, storeThreadPage } = await import(
  "./storeThreadReplies.ts"
);
const {
  loadNewestThreadReplies,
  loadOlderThreadReplies,
  threadWindowStateKey,
} = await import("./threadWindowCache.ts");
const { threadRepliesKey } = await import("./messageQueryKeys.ts");
const { preloadThreads } = await import("../channelPreload.ts");

const clients = [];
async function client({ lastMessageAt = iso(100) } = {}) {
  const queryClient = new QueryClient();
  clients.push(queryClient);
  queryClient.setQueryData(
    ["channels"],
    [{ id: ROOM, lastMessageAt, name: "general" }],
  );
  await hydrateChannelHeads(queryClient, SCOPE);
  calls = [];
  return queryClient;
}

const commands = () => calls.map((call) => call.command);
const contents = (queryClient) =>
  queryClient
    .getQueryData(threadRepliesKey(ROOM, ROOT))
    ?.map((event) => event.content);

afterEach(() => {
  for (const queryClient of clients.splice(0)) queryClient.clear();
});

beforeEach(() => {
  readStore = () => storePage(1, 0);
  readRelay = () => relayPage(1, 3, false);
});

test("a stored thread head is proven only without a gap and up to the room's last activity", () => {
  const page = { gap: false, liveCovered: false, coveredThrough: 100 };
  assert.equal(isProvenThreadHead(page, iso(100)), true);
  assert.equal(isProvenThreadHead(page, iso(100.9)), true);
  assert.equal(isProvenThreadHead(page, iso(101)), false);
  assert.equal(isProvenThreadHead(page, null), false);
  assert.equal(isProvenThreadHead({ ...page, gap: true }, iso(1)), false);
  assert.equal(
    isProvenThreadHead(
      { gap: false, liveCovered: true, coveredThrough: null },
      null,
    ),
    true,
  );
  assert.equal(
    isProvenThreadHead({ ...page, coveredThrough: null }, iso(1)),
    false,
  );
});

test("store pages map onto thread window pages with their overlays", () => {
  const reaction = { ...reply(9), kind: 7, tags: [["e", reply(3).id]] };
  const mapped = storeThreadPage(
    storePage(2, 3, {
      aux: [reaction],
      hasMore: true,
      nextCursor: cursorOf(2),
    }),
    ROOM,
    ROOT,
  );
  assert.deepEqual(
    mapped.events.map((event) => event.id),
    [reply(3).id, reply(2).id, reaction.id],
  );
  assert.equal(mapped.hasMore, true);
  assert.deepEqual(mapped.nextCursor, cursorOf(2));
});

test("a proven stored thread opens with no relay request", async () => {
  const queryClient = await client();
  readStore = () => storePage(4, 6, { hasMore: true, nextCursor: cursorOf(4) });
  const events = await loadNewestThreadReplies(queryClient, ROOM, ROOT);
  assert.deepEqual(
    events.map((event) => event.content),
    ["r4", "r5", "r6"],
  );
  assert.deepEqual(commands(), ["message_store_read_thread_head"]);
  assert.deepEqual(calls[0].args, {
    scope: SCOPE,
    rootId: ROOT,
    before: null,
    limit: 50,
  });
  assert.deepEqual(queryClient.getQueryData(threadWindowStateKey(ROOM, ROOT)), {
    hasOlder: true,
    olderCursor: cursorOf(4),
    newestCovered: cursorOf(6),
  });
});

test("a stale, gapped or unknown stored thread asks the relay", async () => {
  for (const [label, page, lastMessageAt] of [
    ["stale", storePage(1, 2, { coveredThrough: 50 }), iso(100)],
    ["gap", storePage(1, 2, { gap: true }), iso(1)],
    ["unknown", storePage(1, 2), null],
  ]) {
    const queryClient = await client({ lastMessageAt });
    readStore = () => page;
    const events = await loadNewestThreadReplies(queryClient, ROOM, ROOT);
    assert.deepEqual(
      events.map((event) => event.content),
      ["r1", "r2", "r3"],
      label,
    );
    assert.deepEqual(
      commands(),
      ["message_store_read_thread_head", "get_thread_windows"],
      label,
    );
  }
});

test("an explicit room activity time overrides the channel list", async () => {
  const queryClient = await client({ lastMessageAt: iso(500) });
  readStore = () => storePage(1, 2);
  await loadNewestThreadReplies(queryClient, ROOM, ROOT, {
    lastMessageAt: iso(90),
  });
  assert.deepEqual(commands(), ["message_store_read_thread_head"]);
});

test("older replies come from the store, then from the relay below it", async () => {
  const queryClient = await client();
  readStore = ({ before }) =>
    before === null
      ? storePage(8, 9, { hasMore: true, nextCursor: cursorOf(8) })
      : before.createdAt === 8
        ? storePage(6, 7, { hasMore: true, nextCursor: cursorOf(6), gap: true })
        : storePage(1, 0, { gap: true, nextCursor: before, hasMore: true });
  readRelay = () => relayPage(4, 5, true);
  // The thread query stores what the newest load returns.
  queryClient.setQueryData(
    threadRepliesKey(ROOM, ROOT),
    await loadNewestThreadReplies(queryClient, ROOM, ROOT),
  );
  calls = [];
  await loadOlderThreadReplies(queryClient, ROOM, ROOT);
  assert.deepEqual(commands(), ["message_store_read_thread_head"]);
  assert.deepEqual(contents(queryClient), ["r6", "r7", "r8", "r9"]);
  calls = [];
  // The stored run ends at reply 6: the next page is the relay's.
  await loadOlderThreadReplies(queryClient, ROOM, ROOT);
  assert.deepEqual(commands(), [
    "message_store_read_thread_head",
    "get_thread_windows",
  ]);
  assert.deepEqual(calls[1].args.threads[0].cursor, {
    created_at: 6,
    event_id: reply(6).id,
  });
  assert.deepEqual(contents(queryClient), ["r4", "r5", "r6", "r7", "r8", "r9"]);
  assert.equal(
    queryClient.getQueryData(threadWindowStateKey(ROOM, ROOT)).hasOlder,
    true,
  );
});

test("a link target is walked to through stored older pages first", async () => {
  const queryClient = await client();
  readStore = ({ before }) =>
    before === null
      ? storePage(8, 9, { hasMore: true, nextCursor: cursorOf(8) })
      : storePage(6, 7);
  const events = await loadNewestThreadReplies(queryClient, ROOM, ROOT, {
    mustInclude: reply(6).id,
  });
  assert.deepEqual(
    events.map((event) => event.content),
    ["r6", "r7", "r8", "r9"],
  );
  assert.deepEqual(commands(), [
    "message_store_read_thread_head",
    "message_store_read_thread_head",
  ]);
});

test("a link target the proven store lacks is asked of the server", async () => {
  const queryClient = await client();
  readStore = () => storePage(8, 9);
  readRelay = () => relayPage(7, 10, false);
  const events = await loadNewestThreadReplies(queryClient, ROOM, ROOT, {
    mustInclude: reply(10).id,
  });
  assert.deepEqual(commands(), [
    "message_store_read_thread_head",
    "get_thread_windows",
  ]);
  assert.ok(events.some((event) => event.id === reply(10).id));
});

test("thread preload seeds proven threads from the store and asks the relay for the rest", async () => {
  const queryClient = await client();
  const otherRoot = "e".repeat(64);
  readStore = ({ rootId }) =>
    rootId === ROOT ? storePage(1, 2) : storePage(1, 2, { gap: true });
  readRelay = (args) => ({
    ...relayPage(1, 3, false),
    root_id: args.threads[0].rootId,
  });
  const loaded = await preloadThreads(queryClient, [
    { channelId: ROOM, rootId: ROOT },
    { channelId: ROOM, rootId: otherRoot },
  ]);
  assert.equal(loaded, 2);
  assert.deepEqual(contents(queryClient), ["r1", "r2"]);
  const windows = calls.filter((call) => call.command === "get_thread_windows");
  assert.equal(windows.length, 1);
  assert.deepEqual(
    windows[0].args.threads.map((thread) => thread.rootId),
    [otherRoot],
  );
  assert.equal(windows[0].args.limitRows, 20);
  assert.equal(
    calls.find((call) => call.command === "message_store_read_thread_head").args
      .limit,
    20,
  );
});
