import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";

// The Tauri bridge reads window.__TAURI_INTERNALS__.
globalThis.window ??= globalThis;
// The persisted-head cache is enabled unless localStorage turns it off.
globalThis.localStorage ??= { getItem: () => null };

/** @type {Array<{ command: string, args: any }>} */
const calls = [];
let failWindow = false;
/** @type {any[]} */
let threadReplies = [];

let failBatch = false;
/** @type {Set<string>} */
const inaccessible = new Set();

function windowEvents(channelId) {
  return [
    {
      content: "hello",
      created_at: 100,
      id: "a".repeat(64),
      kind: 9,
      pubkey: "b".repeat(64),
      sig: "",
      tags: [["h", channelId]],
    },
    {
      content: JSON.stringify({ has_more: false, next_cursor: null }),
      created_at: 101,
      id: "e".repeat(64),
      kind: 39006,
      pubkey: "f".repeat(64),
      sig: "",
      tags: [
        ["h", channelId],
        ["d", `${channelId}:head`],
      ],
    },
  ];
}

globalThis.__TAURI_INTERNALS__ = {
  invoke: (command, args) => {
    calls.push({ command, args });
    if (command === "get_channel_window") {
      if (failWindow) return Promise.reject(new Error("relay down"));
      return Promise.resolve(windowEvents(args.channelId));
    }
    if (command === "get_channel_windows") {
      if (failBatch) return Promise.reject(new Error("batch down"));
      return Promise.resolve(
        args.channelIds
          .filter((channelId) => !inaccessible.has(channelId))
          .map((channelId) => ({
            channel_id: channelId,
            events: windowEvents(channelId),
          })),
      );
    }
    if (command === "get_event") {
      return Promise.resolve(
        JSON.stringify({
          content: "root",
          created_at: 50,
          id: args.eventId,
          kind: 9,
          pubkey: "b".repeat(64),
          sig: "",
          tags: [["h", ROOM]],
        }),
      );
    }
    if (command === "get_thread_replies") {
      return Promise.resolve({ events: threadReplies, next_cursor: null });
    }
    if (command === "channel_head_cache_load") return Promise.resolve([]);
    if (command === "channel_head_cache_store") return Promise.resolve(null);
    return Promise.reject(new Error(`unmocked Tauri command: ${command}`));
  },
  transformCallback: () => Math.random(),
};

const { QueryClient } = await import("@tanstack/react-query");
const {
  preloadChannelMessages,
  preloadChannelsMessages,
  preloadThreadReplies,
} = await import("./channelPreload.ts");
const { getRememberedRouteEvent } = await import("./lib/routeEventCache.ts");
const { hasPersistedHydratedChannel, hydrateChannelHeads } = await import(
  "./lib/channelHeadCache.ts"
);
const { channelMessagesKey, channelWindowKey, threadRepliesKey } = await import(
  "./lib/messageQueryKeys.ts"
);

const ROOM = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
const ROOT = "d".repeat(64);

/** @type {import("@tanstack/react-query").QueryClient[]} */
const clients = [];
function newClient() {
  const client = new QueryClient();
  clients.push(client);
  return client;
}

beforeEach(() => {
  calls.length = 0;
  failWindow = false;
  failBatch = false;
  inaccessible.clear();
  threadReplies = [];
});

// Cached queries hold gc timers that would keep the test process alive.
afterEach(() => {
  for (const client of clients.splice(0)) client.clear();
});

test("loads a cold room's newest page and lets it paint on open", async () => {
  const client = newClient();
  assert.equal(await preloadChannelMessages(client, ROOM, 20), true);
  assert.deepEqual(
    calls.map((call) => [call.command, call.args.limitRows]),
    [["get_channel_window", 20]],
  );
  assert.equal(client.getQueryData(channelMessagesKey(ROOM)).length, 1);
  assert.equal(hasPersistedHydratedChannel(client, ROOM), true);
});

test("background loads never rank as visits in the persisted-head cache", async () => {
  const client = newClient();
  await hydrateChannelHeads(client, {
    pubkey: "a".repeat(64),
    relayUrl: "wss://r",
  });
  assert.equal(await preloadChannelMessages(client, ROOM, 20), true);
  assert.deepEqual(
    calls.map((call) => call.command),
    ["channel_head_cache_load", "get_channel_window"],
  );
});

test("leaves rooms that already have messages cached alone", async () => {
  const client = newClient();
  client.setQueryData(channelMessagesKey(ROOM), []);
  assert.equal(await preloadChannelMessages(client, ROOM, 20), false);
  assert.equal(calls.length, 0);
  // Nothing to paint: an empty copy keeps the skeleton on open.
  assert.equal(hasPersistedHydratedChannel(client, ROOM), false);
});

test("a room the relay already delivered paints that copy on open", async () => {
  const client = newClient();
  const [row] = windowEvents(ROOM);
  client.setQueryData(channelMessagesKey(ROOM), [row]);
  client.setQueryData(channelWindowKey(ROOM), {
    pages: [
      {
        startCursor: null,
        rows: [{ event: row, thread: null }],
        aux: [],
        nextCursor: null,
        hasMore: false,
      },
    ],
    liveOverlay: [],
    liveAux: [],
    liveSummaries: {},
  });
  assert.equal(await preloadChannelMessages(client, ROOM, 20), false);
  assert.equal(calls.length, 0);
  assert.equal(hasPersistedHydratedChannel(client, ROOM), true);
});

test("a room seeded only by live events keeps its skeleton", async () => {
  const client = newClient();
  client.setQueryData(channelMessagesKey(ROOM), windowEvents(ROOM).slice(0, 1));
  assert.equal(await preloadChannelMessages(client, ROOM, 20), false);
  assert.equal(calls.length, 0);
  assert.equal(hasPersistedHydratedChannel(client, ROOM), false);
});

test("several rooms load in one batched request", async () => {
  const client = newClient();
  const other = "11111111-2222-4333-8444-555555555555";
  const denied = "66666666-7777-4888-9999-000000000000";
  inaccessible.add(denied);
  const loaded = await preloadChannelsMessages(
    client,
    [ROOM, other, denied],
    20,
  );
  assert.deepEqual(loaded, [ROOM, other]);
  assert.deepEqual(
    calls.map((call) => call.command),
    ["get_channel_windows"],
  );
  assert.deepEqual(calls[0].args, {
    channelIds: [ROOM, other, denied],
    limitRows: 20,
  });
  assert.equal(client.getQueryData(channelMessagesKey(other)).length, 1);
  assert.equal(hasPersistedHydratedChannel(client, other), true);
  assert.equal(client.getQueryData(channelMessagesKey(denied)), undefined);
});

test("a failed batch loads nothing and leaves the retry to a later pass", async () => {
  const client = newClient();
  const other = "11111111-2222-4333-8444-555555555555";
  failBatch = true;
  const loaded = await preloadChannelsMessages(client, [ROOM, other], 20);
  assert.deepEqual(loaded, []);
  assert.deepEqual(
    calls.map((call) => call.command),
    ["get_channel_windows"],
  );
  assert.equal(client.getQueryData(channelMessagesKey(ROOM)), undefined);
});

test("a failed load caches nothing and does not mark the room paintable", async () => {
  const client = newClient();
  failWindow = true;
  assert.equal(await preloadChannelMessages(client, ROOM, 20), false);
  assert.equal(client.getQueryData(channelMessagesKey(ROOM)), undefined);
  assert.equal(hasPersistedHydratedChannel(client, ROOM), false);
});

test("thread replies load once and include the reply the row opens on", async () => {
  const client = newClient();
  const reply = {
    content: "reply",
    created_at: 200,
    id: "f".repeat(64),
    kind: 9,
    pubkey: "b".repeat(64),
    sig: "",
    tags: [
      ["h", ROOM],
      ["e", ROOT, "", "root"],
    ],
  };
  threadReplies = [reply];
  assert.equal(await preloadThreadReplies(client, ROOM, ROOT, reply.id), true);
  assert.equal(client.getQueryData(threadRepliesKey(ROOM, ROOT)).length, 1);
  // Opening the row's link resolves the reply and root without the relay.
  assert.equal(getRememberedRouteEvent(client, reply.id)?.id, reply.id);
  assert.equal(getRememberedRouteEvent(client, ROOT)?.id, ROOT);
  assert.equal(await preloadThreadReplies(client, ROOM, ROOT, reply.id), false);
  assert.equal(
    calls.filter((call) => call.command === "get_thread_replies").length,
    1,
  );
});

test("a thread load that misses the opened reply is not cached as complete", async () => {
  const client = newClient();
  assert.equal(
    await preloadThreadReplies(client, ROOM, ROOT, "9".repeat(64)),
    false,
  );
  assert.equal(client.getQueryData(threadRepliesKey(ROOM, ROOT)), undefined);
});
