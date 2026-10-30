import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

import { QueryClient } from "@tanstack/react-query";

import { setDeviceSnapshotDeps } from "../../shared/api/deviceSnapshot.ts";
import {
  CHAT_ROWS_SNAPSHOT_ROWS,
  chatRowsSnapshotKey,
  chatRowsToShow,
  flushChatRowsSnapshot,
  isKeptChatRows,
  keptRowsWithLocalReads,
  parseChatRowsSnapshot,
  prefetchChatRowsSnapshot,
  resetChatRowsSnapshotWriter,
  saveChatRowsSnapshot,
} from "./chatRowsSnapshot.ts";

const ME = "aa".repeat(32);
const OTHER = "bb".repeat(32);
const RELAY = "wss://relay.example";

function row(key, extra = {}) {
  return {
    type: "room",
    key,
    at: 100,
    at_us: 100_000_000,
    channel_id: key,
    channel_type: "stream",
    channel_name: `Room ${key}`,
    archived: false,
    dm_members: [],
    root_id: null,
    thread_name: null,
    follow_state: null,
    closed: false,
    last_seq: 3,
    read_seq: 1,
    unread: 2,
    unread_overflow: false,
    unread_mentions: 0,
    unread_mentions_overflow: false,
    unread_mark: false,
    latest: null,
    root: null,
    ...extra,
  };
}

/** A store answering only for the account and server it was written for. */
function installStore() {
  const blobs = new Map();
  const key = (kind, s) => `${kind}|${s.relayUrl}|${s.pubkey.toLowerCase()}`;
  const writes = [];
  setDeviceSnapshotDeps({
    read: async (kind, s) => blobs.get(key(kind, s)) ?? null,
    write: async (kind, s, blob) => {
      writes.push({ kind, scope: s, blob });
      blobs.set(key(kind, s), blob);
      return true;
    },
  });
  return { blobs, key, writes };
}

afterEach(() => {
  resetChatRowsSnapshotWriter();
  setDeviceSnapshotDeps();
});

test("kept rows draw before the server answers, then the server's replace them", () => {
  const kept = [row("a"), row("b")];
  const server = [row("b"), row("c")];
  // Capability not probed yet this run: the kept rows.
  assert.equal(chatRowsToShow("unknown", undefined, kept), kept);
  // On, first page still out: the kept rows.
  assert.equal(chatRowsToShow("on", undefined, kept), kept);
  // The server answered: its rows, in its order.
  assert.equal(chatRowsToShow("on", server, kept), server);
  // The relay turned chat state off: none.
  assert.equal(chatRowsToShow("off", undefined, kept), undefined);
  // Nothing kept: nothing until the server answers.
  assert.equal(chatRowsToShow("unknown", undefined, []), undefined);
});

test("rows saved for an account are read back at launch for it only", async () => {
  const store = installStore();
  const scope = { pubkey: ME, relayUrl: RELAY };
  const rows = Array.from({ length: CHAT_ROWS_SNAPSHOT_ROWS + 5 }, (_, i) =>
    row(`r${i}`),
  );
  saveChatRowsSnapshot(scope, rows);
  flushChatRowsSnapshot(scope);
  await Promise.resolve();
  assert.equal(store.writes.length, 1);
  assert.equal(store.writes[0].kind, "chat_rows");

  const client = new QueryClient();
  prefetchChatRowsSnapshot(client, scope);
  prefetchChatRowsSnapshot(client, { pubkey: OTHER, relayUrl: RELAY });
  prefetchChatRowsSnapshot(client, { pubkey: ME, relayUrl: "wss://two" });
  await new Promise((resolve) => setTimeout(resolve, 0));
  const mine = client.getQueryData(chatRowsSnapshotKey(RELAY, ME));
  assert.equal(mine.length, CHAT_ROWS_SNAPSHOT_ROWS);
  assert.equal(mine[0].key, "r0");
  assert.equal(client.getQueryData(chatRowsSnapshotKey(RELAY, OTHER)), null);
  assert.equal(client.getQueryData(chatRowsSnapshotKey("wss://two", ME)), null);
});

test("an unchanged list is not written again", async () => {
  const store = installStore();
  const scope = { pubkey: ME, relayUrl: RELAY };
  saveChatRowsSnapshot(scope, [row("a")]);
  flushChatRowsSnapshot(scope);
  saveChatRowsSnapshot(scope, [row("a")]);
  flushChatRowsSnapshot(scope);
  await Promise.resolve();
  assert.equal(store.writes.length, 1);
});

test("malformed or foreign blobs give no rows", () => {
  assert.equal(parseChatRowsSnapshot(null), null);
  assert.equal(parseChatRowsSnapshot("not json"), null);
  assert.equal(parseChatRowsSnapshot('{"version":2,"rows":[]}'), null);
  assert.deepEqual(
    parseChatRowsSnapshot(
      JSON.stringify({ version: 1, rows: [{ key: 1 }, row("ok")] }),
    ).map((kept) => kept.key),
    ["ok"],
  );
});

test("kept rows take this device's reads over their counts and have no actions", () => {
  const kept = [
    row("read", { latest: { created_at: 50 }, unread_mark: true }),
    row("unread", { latest: { created_at: 90 } }),
    row("never", { latest: { created_at: 90 } }),
  ];
  const readAt = { read: 60, unread: 80 };
  const shown = keptRowsWithLocalReads(kept, (id) => readAt[id] ?? null);
  assert.equal(shown[0].unread, 0, "read here since: no badge");
  assert.equal(shown[0].unread_mark, false);
  assert.equal(shown[1], kept[1], "a newer message stays unread");
  assert.equal(shown[2], kept[2]);
  assert.ok(isKeptChatRows(shown));
  assert.equal(isKeptChatRows(kept), false);
});
