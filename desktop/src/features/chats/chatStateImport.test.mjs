import assert from "node:assert/strict";
import test from "node:test";

import {
  buildChatImportChunks,
  CHAT_IMPORT_MAX_ITEMS,
  chatImportFlagKey,
  ChatStateImporter,
} from "./chatStateImport.ts";

const PUBKEY = "a".repeat(64);
const RELAY = "wss://tenant-a.example";
const KEY = `${RELAY}|${PUBKEY}`;
const BINDING = { pubkey: PUBKEY, relayUrl: RELAY };
const OTHER = { pubkey: "b".repeat(64), relayUrl: RELAY };
const OTHER_KEY = `${RELAY}|${OTHER.pubkey}`;
const NOW = 1_760_000_000;

function roomId(n) {
  return `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
}

function rootId(n) {
  return n.toString(16).padStart(64, "0");
}

function memoryStorage() {
  const values = new Map();
  return {
    values,
    getItem: (key) => (values.has(key) ? values.get(key) : null),
    setItem: (key, value) => values.set(key, String(value)),
  };
}

function sources(overrides = {}) {
  return {
    contexts: { [roomId(1)]: NOW - 10, [`thread:${rootId(1)}`]: NOW - 5 },
    followedRootIds: [rootId(1)],
    mutedRootIds: [],
    ...overrides,
  };
}

function importer({ send, storage = memoryStorage(), unavailable = [] } = {}) {
  const sent = [];
  const imported = [];
  const instance = new ChatStateImporter({
    send: (binding, chunk) => {
      sent.push({ binding, chunk });
      return send ? send(binding, chunk, sent.length) : Promise.resolve({});
    },
    onUnavailable: (key) => unavailable.push(key),
    onImported: (key) => imported.push(key),
    storage,
    nowSeconds: () => NOW,
  });
  return { instance, sent, imported, storage };
}

function input(overrides = {}) {
  return {
    key: KEY,
    binding: BINDING,
    capability: "on",
    readStateInitialized: true,
    collect: () => sources(),
    ...overrides,
  };
}

test("reads skip msg: contexts and future times; a muted root is unfollowed", () => {
  const [chunk] = buildChatImportChunks(
    {
      contexts: {
        [roomId(1)]: NOW + 3_600,
        [`msg:${rootId(9)}`]: NOW,
        "not-a-room": NOW,
        [`thread:${rootId(2).toUpperCase()}`]: NOW - 1,
      },
      followedRootIds: [rootId(2), rootId(3), "short"],
      mutedRootIds: [rootId(3), rootId(4)],
    },
    NOW,
  );
  assert.deepEqual(chunk.reads, [
    { kind: "channel", id: roomId(1), at: NOW },
    { kind: "thread", id: rootId(2), at: NOW - 1 },
  ]);
  assert.deepEqual(chunk.follows, [
    { root: rootId(2), state: "followed" },
    { root: rootId(3), state: "unfollowed" },
    { root: rootId(4), state: "unfollowed" },
  ]);
});

test("chunks hold at most 500 items, deduped, a thread's read and follow together", () => {
  const contexts = {};
  for (let n = 1; n <= 499; n += 1) contexts[roomId(n)] = NOW - n;
  // The same room twice (case differs): the newest time wins, once.
  contexts[roomId(1).toUpperCase()] = NOW;
  contexts[`thread:${rootId(1)}`] = NOW - 1;
  const chunks = buildChatImportChunks(
    {
      contexts,
      followedRootIds: [rootId(1), rootId(1), rootId(2)],
      mutedRootIds: [],
    },
    NOW,
  );
  const sizes = chunks.map((c) => c.reads.length + c.follows.length);
  assert.deepEqual(sizes, [499, 3]);
  assert.ok(sizes.every((size) => size <= CHAT_IMPORT_MAX_ITEMS));
  const rooms = chunks
    .flatMap((c) => c.reads)
    .filter((r) => r.kind === "channel");
  assert.equal(rooms.length, 499);
  assert.equal(rooms.find((r) => r.id === roomId(1)).at, NOW);
  // Thread 1's read and follow went to the second chunk together.
  assert.deepEqual(chunks[1], {
    reads: [{ kind: "thread", id: rootId(1), at: NOW - 1 }],
    follows: [
      { root: rootId(1), state: "followed" },
      { root: rootId(2), state: "followed" },
    ],
  });
  assert.deepEqual(
    buildChatImportChunks(sources({ contexts: {}, followedRootIds: [] }), NOW),
    [],
  );
});

test("every chunk with threads repeats the room reads, so threads get raised", () => {
  const contexts = { [roomId(1)]: NOW - 1, [roomId(2)]: NOW - 2 };
  for (let n = 1; n <= 6; n += 1) contexts[`thread:${rootId(n)}`] = NOW - 9;
  const chunks = buildChatImportChunks(
    { contexts, followedRootIds: [rootId(1)], mutedRootIds: [] },
    NOW,
    5,
  );
  const rooms = [
    { kind: "channel", id: roomId(1), at: NOW - 1 },
    { kind: "channel", id: roomId(2), at: NOW - 2 },
  ];
  // Thread 1 is a read plus a follow (two items), the rest one read each.
  assert.deepEqual(
    chunks.map((c) => [c.reads.length, c.follows.length]),
    [
      [4, 1],
      [5, 0],
      [3, 0],
    ],
  );
  for (const chunk of chunks) {
    assert.deepEqual(chunk.reads.slice(0, 2), rooms);
    assert.ok(chunk.reads.length + chunk.follows.length <= 5);
  }
  const threads = chunks.flatMap((c) => c.reads.slice(2).map((r) => r.id));
  assert.deepEqual(threads, [1, 2, 3, 4, 5, 6].map(rootId));
  // Room reads alone are never repeated.
  assert.deepEqual(
    buildChatImportChunks(
      { contexts: { [roomId(1)]: NOW }, followedRootIds: [], mutedRootIds: [] },
      NOW,
      5,
    ).map((c) => c.reads.length),
    [1],
  );
});

test("waits until the local read markers are complete", async () => {
  const { instance, sent } = importer();
  assert.equal(instance.update(input({ readStateInitialized: false })), null);
  assert.equal(sent.length, 0);
  assert.equal(await instance.update(input()), "done");
  assert.equal(sent.length, 1);
  assert.deepEqual(sent[0].binding, BINDING);
});

test("never runs while the capability is off or unknown", () => {
  const { instance, sent } = importer();
  assert.equal(instance.update(input({ capability: "off" })), null);
  assert.equal(instance.update(input({ capability: "unknown" })), null);
  assert.equal(instance.update(input({ key: null, binding: null })), null);
  assert.equal(sent.length, 0);
});

test("the done flag is set only when every chunk was accepted", async () => {
  const many = {};
  for (let n = 1; n <= 600; n += 1) many[roomId(n)] = NOW - n;
  const collect = () => sources({ contexts: many, followedRootIds: [] });
  const storage = memoryStorage();
  const flag = chatImportFlagKey(PUBKEY, RELAY);

  const first = importer({
    storage,
    send: (_b, _c, call) =>
      call === 2
        ? Promise.reject(new Error("relay unreachable"))
        : Promise.resolve({}),
  });
  assert.equal(await first.instance.update(input({ collect })), "failed");
  assert.equal(first.sent.length, 2);
  assert.equal(storage.getItem(flag), null);
  assert.deepEqual(first.imported, [KEY], "the stored part is shown");
  // Attempted once per start, even as inputs keep changing.
  assert.equal(first.instance.update(input({ collect })), null);

  // A later start retries everything and then marks the account done.
  const later = importer({ storage });
  assert.equal(await later.instance.update(input({ collect })), "done");
  assert.equal(later.sent.length, 2);
  assert.equal(storage.getItem(flag), "1");
  assert.deepEqual(later.imported, [KEY], "the list is refetched once");

  const after = importer({ storage });
  assert.equal(after.instance.update(input({ collect })), null);
  assert.equal(after.sent.length, 0);
});

test("an account switch mid-import stops before the next chunk", async () => {
  const many = {};
  for (let n = 1; n <= 600; n += 1) many[roomId(n)] = NOW - n;
  let release;
  const storage = memoryStorage();
  const { instance, sent, imported } = importer({
    storage,
    send: () => new Promise((resolve) => (release = resolve)),
  });
  const run = instance.update(
    input({ collect: () => sources({ contexts: many, followedRootIds: [] }) }),
  );
  assert.equal(sent.length, 1);
  instance.update(
    input({ key: OTHER_KEY, binding: OTHER, readStateInitialized: false }),
  );
  release({});
  assert.equal(await run, "stopped");
  assert.equal(sent.length, 1);
  assert.equal(storage.getItem(chatImportFlagKey(PUBKEY, RELAY)), null);
  assert.deepEqual(imported, []);
});

test("refused for another account or unavailable: no flag", async () => {
  const storage = memoryStorage();
  const unavailable = [];
  const changed = importer({
    storage,
    send: () => Promise.reject(new Error("chat_state_account_changed")),
  });
  assert.equal(await changed.instance.update(input()), "stopped");
  const off = importer({
    storage,
    unavailable,
    send: () => Promise.reject(new Error("chat_state_unavailable")),
  });
  assert.equal(await off.instance.update(input()), "unavailable");
  assert.deepEqual(unavailable, [KEY]);
  assert.equal(storage.getItem(chatImportFlagKey(PUBKEY, RELAY)), null);
});
