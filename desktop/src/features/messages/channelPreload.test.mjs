import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";

// The Tauri bridge reads window.__TAURI_INTERNALS__.
globalThis.window ??= globalThis;

/** @type {Array<{ command: string, args: any }>} */
const calls = [];
let failWindow = false;

globalThis.__TAURI_INTERNALS__ = {
  invoke: (command, args) => {
    calls.push({ command, args });
    if (command === "get_channel_window") {
      if (failWindow) return Promise.reject(new Error("relay down"));
      return Promise.resolve([
        {
          content: "hello",
          created_at: 100,
          id: "a".repeat(64),
          kind: 9,
          pubkey: "b".repeat(64),
          sig: "",
          tags: [["h", args.channelId]],
        },
        {
          content: JSON.stringify({ has_more: false, next_cursor: null }),
          created_at: 101,
          id: "e".repeat(64),
          kind: 39006,
          pubkey: "f".repeat(64),
          sig: "",
          tags: [
            ["h", args.channelId],
            ["d", `${args.channelId}:head`],
          ],
        },
      ]);
    }
    if (command === "get_thread_replies") {
      return Promise.resolve({ events: [], next_cursor: null });
    }
    return Promise.reject(new Error(`unmocked Tauri command: ${command}`));
  },
  transformCallback: () => Math.random(),
};

const { QueryClient } = await import("@tanstack/react-query");
const { preloadChannelMessages, preloadThreadReplies } = await import(
  "./channelPreload.ts"
);
const { hasPersistedHydratedChannel } = await import(
  "./lib/channelHeadCache.ts"
);
const { channelMessagesKey, threadRepliesKey } = await import(
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

test("leaves rooms that already have messages cached alone", async () => {
  const client = newClient();
  client.setQueryData(channelMessagesKey(ROOM), []);
  assert.equal(await preloadChannelMessages(client, ROOM, 20), false);
  assert.equal(calls.length, 0);
});

test("a failed load caches nothing and does not mark the room paintable", async () => {
  const client = newClient();
  failWindow = true;
  assert.equal(await preloadChannelMessages(client, ROOM, 20), false);
  assert.equal(client.getQueryData(channelMessagesKey(ROOM)), undefined);
  assert.equal(hasPersistedHydratedChannel(client, ROOM), false);
});

test("thread replies load once", async () => {
  const client = newClient();
  assert.equal(await preloadThreadReplies(client, ROOM, ROOT), true);
  assert.deepEqual(client.getQueryData(threadRepliesKey(ROOM, ROOT)), []);
  assert.equal(await preloadThreadReplies(client, ROOM, ROOT), false);
  assert.equal(
    calls.filter((call) => call.command === "get_thread_replies").length,
    1,
  );
});
