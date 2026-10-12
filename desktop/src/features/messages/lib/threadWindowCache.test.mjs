import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

// The Tauri bridge reads window.__TAURI_INTERNALS__.
globalThis.window ??= globalThis;

/** @type {Array<{ command: string, args: any }>} */
const calls = [];
/** @type {Array<(args: any) => any>} */
let pages = [];

globalThis.__TAURI_INTERNALS__ = {
  invoke: (command, args) => {
    calls.push({ command, args });
    if (command === "get_thread_windows") {
      const next = pages.shift();
      return Promise.resolve(next ? [next(args)] : []);
    }
    return Promise.reject(new Error(`unmocked Tauri command: ${command}`));
  },
  transformCallback: () => Math.random(),
};

const { QueryClient } = await import("@tanstack/react-query");
const {
  loadNewestThreadReplies,
  loadOlderThreadReplies,
  mergeNewestThreadPage,
  threadWindowStateKey,
} = await import("./threadWindowCache.ts");
const { threadRepliesKey } = await import("./messageQueryKeys.ts");

const ROOM = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
const ROOT = "d".repeat(64);

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

/** A relay page holding replies `from`..`to` (inclusive), newest first. */
function page(from, to, hasMore) {
  const events = [];
  for (let n = to; n >= from; n -= 1) events.push(reply(n));
  const oldest = reply(from);
  return {
    channelId: ROOM,
    rootId: ROOT,
    events,
    hasMore,
    nextCursor: hasMore
      ? { createdAt: oldest.created_at, eventId: oldest.id }
      : null,
  };
}

function rawPage(from, to, hasMore) {
  const p = page(from, to, hasMore);
  return () => ({
    channel_id: ROOM,
    root_id: ROOT,
    events: p.events,
    has_more: p.hasMore,
    next_cursor: p.nextCursor
      ? { created_at: p.nextCursor.createdAt, event_id: p.nextCursor.eventId }
      : null,
  });
}

const ids = (events) => events.map((event) => event.created_at);

/** @type {import("@tanstack/react-query").QueryClient[]} */
const clients = [];
afterEach(() => {
  calls.length = 0;
  pages = [];
  for (const client of clients.splice(0)) client.clear();
});
function newClient() {
  const client = new QueryClient();
  clients.push(client);
  return client;
}

const cursor = (n) => ({ createdAt: n, eventId: reply(n).id });

test("a cold thread keeps just the newest page, oldest first", () => {
  const { events, state } = mergeNewestThreadPage(
    undefined,
    undefined,
    page(51, 100, true),
  );
  assert.equal(events.length, 50);
  assert.equal(events[0].created_at, 51);
  assert.equal(events.at(-1).created_at, 100);
  assert.deepEqual(state, {
    hasOlder: true,
    olderCursor: cursor(51),
    newestCovered: cursor(100),
  });
});

test("reopening keeps older loaded replies that connect to the new page", () => {
  const cached = [...page(1, 60, false).events].reverse();
  const cachedState = {
    hasOlder: false,
    olderCursor: null,
    newestCovered: cursor(60),
  };
  // Two new replies arrived; the newest page now covers 13..62.
  const { events, state } = mergeNewestThreadPage(
    cached,
    cachedState,
    page(13, 62, true),
  );
  assert.deepEqual(ids(events), Array.from({ length: 62 }, (_, i) => i + 1));
  assert.deepEqual(state, {
    hasOlder: false,
    olderCursor: null,
    newestCovered: cursor(62),
  });
});

test("a page that cannot reach the cached replies restarts from the page", () => {
  const cached = [...page(1, 10, false).events].reverse();
  const { events, state } = mergeNewestThreadPage(
    cached,
    { hasOlder: false, olderCursor: null, newestCovered: cursor(10) },
    page(51, 100, true),
  );
  assert.deepEqual(ids(events), Array.from({ length: 50 }, (_, i) => i + 51));
  assert.equal(state.hasOlder, true);
});

test("a live reply inside the new page does not hide replies missed in a gap", () => {
  // Cached 1..50 (covered to 50); 60 replies were missed, then reply 111
  // arrived live. The newest page covers 62..111 and shares reply 111.
  const cached = [...page(1, 50, false).events].reverse().concat(reply(111));
  const { events, state } = mergeNewestThreadPage(
    cached,
    { hasOlder: false, olderCursor: null, newestCovered: cursor(50) },
    page(62, 111, true),
  );
  assert.deepEqual(ids(events), Array.from({ length: 50 }, (_, i) => i + 62));
  assert.deepEqual(state.olderCursor, cursor(62));
});

test("reactions and deletions in the cache are kept, not mistaken for replies", () => {
  const reaction = { ...reply(55), id: "r".repeat(64), kind: 7 };
  const deletion = { ...reply(57), id: "x".repeat(64), kind: 5 };
  const cached = [...page(1, 60, false).events]
    .reverse()
    .concat(reaction, deletion);
  const { events } = mergeNewestThreadPage(
    cached,
    { hasOlder: false, olderCursor: null, newestCovered: cursor(60) },
    page(13, 62, true),
  );
  assert.ok(events.some((event) => event.id === reaction.id));
  assert.ok(events.some((event) => event.id === deletion.id));
  assert.equal(events.filter((event) => event.kind === 9).length, 62);
});

test("replies deleted inside the page span drop out; live arrivals stay", () => {
  const cached = [reply(5), reply(6), reply(7), reply(9)];
  // The relay no longer has 6; 9 arrived live after the page was cut at 7.
  const fresh = page(5, 7, false);
  fresh.events = fresh.events.filter((event) => event.created_at !== 6);
  const { events } = mergeNewestThreadPage(
    cached,
    { hasOlder: false, olderCursor: null, newestCovered: cursor(7) },
    fresh,
  );
  assert.deepEqual(ids(events), [5, 7, 9]);
});

test("opening loads one newest page; scrolling up loads the page before it", async () => {
  const client = newClient();
  pages = [rawPage(51, 100, true), rawPage(1, 50, false)];
  const events = await loadNewestThreadReplies(client, ROOM, ROOT);
  assert.equal(events.length, 50);
  assert.deepEqual(calls[0].args.threads[0].cursor, null);
  assert.equal(calls[0].args.limitRows, 50);
  client.setQueryData(threadRepliesKey(ROOM, ROOT), events);

  await loadOlderThreadReplies(client, ROOM, ROOT);
  assert.deepEqual(calls[1].args.threads[0].cursor, {
    created_at: 51,
    event_id: reply(51).id,
  });
  const all = client.getQueryData(threadRepliesKey(ROOM, ROOT));
  assert.deepEqual(
    ids(all),
    Array.from({ length: 100 }, (_, i) => i + 1),
  );
  assert.deepEqual(client.getQueryData(threadWindowStateKey(ROOM, ROOT)), {
    hasOlder: false,
    olderCursor: null,
    newestCovered: cursor(100),
  });
  // Nothing older remains: no further request.
  await loadOlderThreadReplies(client, ROOM, ROOT);
  assert.equal(calls.length, 2);
});

test("a link to an older reply walks back until that reply is loaded", async () => {
  const client = newClient();
  pages = [rawPage(51, 100, true), rawPage(1, 50, false)];
  const events = await loadNewestThreadReplies(client, ROOM, ROOT, {
    mustInclude: reply(7).id,
  });
  assert.equal(events.length, 100);
  assert.equal(calls.length, 2);
});

test("a link whose reply is never reached leaves the paging state untouched", async () => {
  const client = newClient();
  pages = [rawPage(51, 100, true), rawPage(1, 50, false)];
  const events = await loadNewestThreadReplies(client, ROOM, ROOT, {
    mustInclude: "9".repeat(64),
  });
  assert.equal(events.length, 100);
  assert.equal(client.getQueryData(threadWindowStateKey(ROOM, ROOT)), undefined);
});

test("an older page is dropped when a newest-page restart moved the cursor", async () => {
  const client = newClient();
  const stateKey = threadWindowStateKey(ROOM, ROOT);
  client.setQueryData(threadRepliesKey(ROOM, ROOT), [reply(60)]);
  client.setQueryData(stateKey, {
    hasOlder: true,
    olderCursor: cursor(60),
    newestCovered: cursor(60),
  });
  let release;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  pages = [
    async (args) => {
      await gate;
      return rawPage(10, 59, true)(args);
    },
  ];
  const older = loadOlderThreadReplies(client, ROOM, ROOT);
  // Meanwhile the cache restarted from a newer page.
  client.setQueryData(threadRepliesKey(ROOM, ROOT), [reply(200)]);
  client.setQueryData(stateKey, {
    hasOlder: true,
    olderCursor: cursor(200),
    newestCovered: cursor(200),
  });
  release();
  await older;
  assert.deepEqual(ids(client.getQueryData(threadRepliesKey(ROOM, ROOT))), [
    200,
  ]);
  assert.deepEqual(client.getQueryData(stateKey).olderCursor, cursor(200));
});

test("no window from the relay resolves to null for the full-read fallback", async () => {
  const client = newClient();
  pages = [];
  assert.equal(await loadNewestThreadReplies(client, ROOM, ROOT), null);
});
