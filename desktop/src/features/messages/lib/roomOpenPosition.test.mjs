import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";

// The Tauri bridge reads window.__TAURI_INTERNALS__.
globalThis.window ??= globalThis;

const ROOM = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
const SCOPE = { pubkey: "f".repeat(64), relayUrl: "wss://relay" };
const iso = (seconds) => new Date(seconds * 1000).toISOString();

/** Message `n` is created at second `n`; a higher n is newer. */
function message(n) {
  return {
    content: `m${n}`,
    created_at: n,
    id: n.toString(16).padStart(64, "0"),
    kind: 9,
    pubkey: "b".repeat(64),
    sig: "",
    tags: [["h", ROOM]],
  };
}
const cursorOf = (n) => ({ createdAt: n, eventId: message(n).id });

/** Messages `from`..`to`, newest first. */
function newestFirst(from, to) {
  const rows = [];
  for (let n = to; n >= from; n -= 1) rows.push(message(n));
  return rows;
}

/** A store page holding messages `from`..`to`. */
function storePage(from, to, before = null) {
  return {
    startCursor: before,
    rows: newestFirst(from, to),
    aux: [],
    summaries: [],
    nextCursor: from > 1 ? cursorOf(from) : null,
    hasMore: from > 1,
    gap: false,
  };
}

/** The server's newest window: messages `from`..`to` and its bounds. */
function serverHead(from, to) {
  const next = from > 1 ? { created_at: from, id: message(from).id } : null;
  return [
    ...newestFirst(from, to),
    {
      content: JSON.stringify({ has_more: next !== null, next_cursor: next }),
      created_at: to,
      id: "d".repeat(64),
      kind: 39006,
      pubkey: "e".repeat(64),
      sig: "",
      tags: [
        ["h", ROOM],
        ["d", `${ROOM}:head`],
      ],
    },
  ];
}

/** A window store whose pages hold the given [from, to] runs, newest first. */
function windowOf(...runs) {
  return {
    pages: runs.map(([from, to], index) => ({
      startCursor: index === 0 ? null : cursorOf(runs[index - 1][0]),
      rows: newestFirst(from, to).map((event) => ({ event, thread: null })),
      aux: [],
      nextCursor: from > 1 ? cursorOf(from) : null,
      hasMore: from > 1,
    })),
    liveOverlay: [],
    liveAux: [],
    liveSummaries: {},
  };
}

let calls = [];
/** Store rows below `before` (or the newest), `PAGE` at a time, down to 1. */
let storedThrough = 100;
let coverageHiAt = 100;
let serverFails = false;
const PAGE = 10;

globalThis.__TAURI_INTERNALS__ = {
  invoke: async (command, args) => {
    calls.push({ command, args });
    switch (command) {
      case "message_store_load_heads":
        return [];
      case "message_store_coverage":
        return [
          {
            channelId: ROOM,
            ranges: [
              {
                class: "top",
                loAt: 0,
                loId: "",
                loIsStart: true,
                hiAt: coverageHiAt,
              },
            ],
            liveCovered: false,
            provenThrough: null,
            newestLocalAt: coverageHiAt,
            gapBelow: false,
          },
        ];
      case "message_store_read_window": {
        const top = args.before ? args.before.createdAt - 1 : storedThrough;
        if (top < 1) return { ...storePage(1, 0, args.before), hasMore: false };
        return storePage(Math.max(1, top - PAGE + 1), top, args.before);
      }
      case "get_channel_window":
        if (serverFails) throw new Error("relay unreachable");
        return serverHead(91, 100);
      default:
        throw new Error(`unmocked Tauri command: ${command}`);
    }
  },
  transformCallback: () => Math.random(),
};

const { QueryClient } = await import("@tanstack/react-query");
const { hydrateChannelHeads, loadRoomHead, withServerRefresh } = await import(
  "./storeChannelHeads.ts"
);
const { storedPagesBelowHead, withStoredBelow } = await import(
  "./pageOlderMessages.ts"
);
const { channelWindowKey } = await import("./messageQueryKeys.ts");
const { preloadUnreadAnchors } = await import("../channelPreload.ts");
const { inboxUnreadAnchorTargets } = await import(
  "../../home/lib/inboxPreload.ts"
);

const clients = [];
async function client() {
  const queryClient = new QueryClient();
  clients.push(queryClient);
  await hydrateChannelHeads(queryClient, SCOPE);
  calls = [];
  return queryClient;
}
const commands = () => calls.map((call) => call.command);
const windowIds = (queryClient) =>
  queryClient
    .getQueryData(channelWindowKey(ROOM))
    .pages.flatMap((page) => page.rows.map((row) => row.event.created_at));

beforeEach(() => {
  storedThrough = 100;
  coverageHiAt = 100;
  serverFails = false;
});
afterEach(() => {
  for (const queryClient of clients.splice(0)) queryClient.clear();
});

test("an open the server cannot answer shows the stored rows unproven", async () => {
  const queryClient = await client();
  coverageHiAt = 50;
  serverFails = true;
  const room = { id: ROOM, lastMessageAt: iso(100) };
  const head = await loadRoomHead(queryClient, room, []);
  assert.deepEqual(
    head.stored.map((event) => event.created_at),
    newestFirst(91, 100)
      .reverse()
      .map((event) => event.created_at),
  );
  // Rows already showing, or a refresh, still fail: the stored copy never
  // replaces what the reader sees.
  await assert.rejects(loadRoomHead(queryClient, room, [message(1)]));
  await assert.rejects(
    withServerRefresh(queryClient, ROOM, () =>
      loadRoomHead(queryClient, room, []),
    ),
  );
});

test("an online open outside proven coverage asks the server", async () => {
  const queryClient = await client();
  coverageHiAt = 50;
  const head = await loadRoomHead(
    queryClient,
    { id: ROOM, lastMessageAt: iso(100) },
    [],
  );
  assert.equal(head.events.length, 11);
  assert.ok(commands().includes("get_channel_window"));
});

test("a head refetch keeps the stored rows the window held below it", async () => {
  const queryClient = await client();
  // Opened at a first unread around 70: head and two older pages.
  queryClient.setQueryData(
    channelWindowKey(ROOM),
    windowOf([91, 100], [81, 90], [71, 80]),
  );
  storedThrough = 102;
  const below = await storedPagesBelowHead(
    queryClient,
    ROOM,
    serverHead(93, 102),
  );
  // Down to the old floor (71), from the store only.
  assert.deepEqual(
    below.map((page) => [page.rows.at(-1).event.created_at, page.hasMore]),
    [
      [83, true],
      [73, true],
      [63, true],
    ],
  );
  assert.ok(!commands().includes("get_channel_window"));
  const head = windowOf([93, 102]);
  const kept = withStoredBelow(head, below);
  assert.equal(kept.pages.length, 4);
  assert.equal(kept.pages.at(-1).rows.at(-1).event.created_at, 63);
  // Pages that do not continue the chain are dropped, never half-applied.
  assert.equal(withStoredBelow(head, below.slice(1)), head);
});

test("a window holding only its head keeps nothing below a refetch", async () => {
  const queryClient = await client();
  queryClient.setQueryData(channelWindowKey(ROOM), windowOf([91, 100]));
  assert.deepEqual(
    await storedPagesBelowHead(queryClient, ROOM, serverHead(91, 100)),
    [],
  );
  assert.deepEqual(calls, []);
});

test("a head refetch retains the old floor within the same second", async (t) => {
  const queryClient = await client();
  const event = (n) => ({ ...message(n), created_at: 100 });
  const cursor = (n) => ({ createdAt: 100, eventId: event(n).id });
  const rows = (from, to) =>
    Array.from({ length: to - from + 1 }, (_, i) => event(from + i));
  const page = (from, to, before) => ({
    startCursor: before,
    rows: rows(from, to).map((event) => ({ event, thread: null })),
    aux: [],
    nextCursor: cursor(to),
    hasMore: true,
  });
  queryClient.setQueryData(channelWindowKey(ROOM), {
    ...windowOf(),
    pages: [page(1, 5, null), page(6, 10, cursor(5))],
  });
  t.mock.method(
    globalThis.__TAURI_INTERNALS__,
    "invoke",
    async (command, args) => {
      assert.equal(command, "message_store_read_window");
      const from = Number.parseInt(args.before.eventId, 16) + 1;
      return {
        rows: rows(from, from + 4),
        aux: [],
        summaries: [],
        nextCursor: cursor(from + 4),
        hasMore: true,
        gap: false,
      };
    },
  );
  const metadata = serverHead(1, 4).at(-1);
  metadata.content = JSON.stringify({
    has_more: true,
    next_cursor: { created_at: 100, id: event(4).id },
  });
  const below = await storedPagesBelowHead(queryClient, ROOM, [
    ...rows(0, 4),
    metadata,
  ]);
  assert.deepEqual(
    below.map((page) => page.nextCursor.eventId),
    [event(9).id, event(14).id],
  );
  assert.ok(
    below.some((page) =>
      page.rows.some((row) => row.event.id === event(10).id),
    ),
  );
});

test("unread preload pages each room down to its read marker from the store", async () => {
  const queryClient = await client();
  queryClient.setQueryData(channelWindowKey(ROOM), windowOf([91, 100]));
  const reached = await preloadUnreadAnchors(queryClient, [
    { channelId: ROOM, readAt: 65 },
    { channelId: "not-loaded", readAt: 1 },
  ]);
  assert.deepEqual(reached, [ROOM]);
  assert.ok(Math.min(...windowIds(queryClient)) <= 65);
  assert.ok(!commands().includes("get_channel_window"));
});

test("unread preload stops when the user is busy", async () => {
  const queryClient = await client();
  queryClient.setQueryData(channelWindowKey(ROOM), windowOf([91, 100]));
  const reached = await preloadUnreadAnchors(
    queryClient,
    [{ channelId: ROOM, readAt: 5 }],
    () => false,
  );
  assert.deepEqual(reached, []);
  assert.equal(Math.min(...windowIds(queryClient)), 91);
});

test("unread anchor targets are the newest rooms with unread and a read marker", () => {
  const row = (channelId, unreadCount, tags = []) => {
    const item = {
      category: "activity",
      channelId,
      channelName: "x",
      channelType: "stream",
      content: "hi",
      createdAt: 100,
      id: `${channelId}-latest`,
      kind: 9,
      pubkey: "a".repeat(64),
      tags,
    };
    return {
      categories: ["activity"],
      conversationId: `channel:${channelId}`,
      groupItems: [item],
      id: item.id,
      isActionRequired: false,
      item,
      unreadCount,
    };
  };
  const rooms = ["r1", "r2", "r3", "r4"].map((id) =>
    id.padEnd(36, "0").replace(/^(.{8})(.{4})(.{4})(.{4})/, "$1-$2-$3-$4-"),
  );
  const readAt = { [rooms[0]]: 10, [rooms[2]]: 30, [rooms[3]]: 40 };
  const targets = inboxUnreadAnchorTargets(
    [row(rooms[0], 2), row(rooms[1], 3), row(rooms[2], 0), row(rooms[3], 1)],
    (id) => readAt[id] ?? null,
  );
  // r2 was never read (opens at its newest), r3 has nothing unread.
  assert.deepEqual(targets, [
    { channelId: rooms[0], readAt: 10 },
    { channelId: rooms[3], readAt: 40 },
  ]);
  assert.equal(
    inboxUnreadAnchorTargets(
      [row(rooms[0], 2), row(rooms[3], 1)],
      (id) => readAt[id] ?? null,
      1,
    ).length,
    1,
  );
});
