import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import test from "node:test";
import { createMockMessageStore } from "./e2eMessageStore.ts";

// The same fixtures run against the Rust store (`message_store/tests.rs`).
const FIXTURES = new URL(
  "../../src-tauri/testdata/message_store/",
  import.meta.url,
);
const SCOPE = { pubkey: "ab".repeat(32), relayUrl: "wss://relay.example" };

function project(value) {
  if (Array.isArray(value)) return value.map(project);
  if (value === null || typeof value !== "object") return value;
  const keys = { rows: "rowIds", aux: "auxIds", summaries: "summaryIds" };
  return Object.fromEntries(
    Object.entries(value).map(([key, entry]) =>
      keys[key] && Array.isArray(entry)
        ? [keys[key], entry.map((event) => event.id)]
        : [key, project(entry)],
    ),
  );
}

function runStep(store, step) {
  switch (step.op) {
    case "ingest":
      store.insertEvents(step.events, step.channelId ?? null);
      return undefined;
    case "claim":
      store.claimRoom(step.channelId, step.class, step);
      return undefined;
    case "claimThread":
      store.claimThread(step.rootId, step);
      return undefined;
    case "beginLive":
      store.beginLive(step.wsId, step.channelIds, step.since);
      return undefined;
    case "closeLive":
      store.closeLive(step.wsId, step.closedAt);
      return undefined;
    case "readWindow":
      return store.readWindow(step.channelId, step.before, step.limit);
    case "readAround":
      return store.readAround(
        step.channelId,
        step.anchor,
        step.before,
        step.after,
      );
    case "readThread":
      return store.readThread(step.rootId, step.cursor, step.limit);
    case "coverage":
      return step.channelIds.map((channelId) => store.coverage(channelId));
    case "loadHeads":
      return store.loadHeads(step.rooms, step.rows);
    default:
      throw new Error(`unknown fixture op ${step.op}`);
  }
}

const names = readdirSync(FIXTURES)
  .filter((name) => name.endsWith(".json"))
  .sort();

test("contract fixtures exist", () => {
  assert.ok(names.length >= 3);
});

for (const name of names) {
  test(`mock message store matches contract fixture ${name}`, () => {
    const fixture = JSON.parse(readFileSync(new URL(name, FIXTURES), "utf8"));
    const store = createMockMessageStore(null).scope(SCOPE);
    fixture.steps.forEach((step, index) => {
      const actual = runStep(store, step);
      if (actual !== undefined) {
        assert.deepEqual(project(actual), step.expect, `${name} step ${index}`);
      }
    });
  });
}

function memoryStorage() {
  const items = new Map();
  return {
    getItem: (key) => items.get(key) ?? null,
    setItem: (key, value) => items.set(key, value),
    removeItem: (key) => items.delete(key),
  };
}

test("reload works like a restart: data persists and open segments close at the last frame", () => {
  const storage = memoryStorage();
  const first = createMockMessageStore(storage).scope(SCOPE);
  first.beginLive(1, ["room"], 100);
  first.noteFrame(150);
  assert.equal(first.coverage("room").ranges.length, 0);
  const reloaded = createMockMessageStore(storage).scope(SCOPE);
  assert.deepEqual(reloaded.coverage("room").ranges, [
    { class: "top", loAt: 100, loId: "~", loIsStart: false, hiAt: 150 },
    { class: "all", loAt: 100, loId: "~", loIsStart: false, hiAt: 150 },
  ]);
});

test("scopes are isolated and clear drops one scope", () => {
  const storage = memoryStorage();
  const stores = createMockMessageStore(storage);
  const other = { ...SCOPE, pubkey: "cd".repeat(32) };
  const event = {
    id: "01".repeat(32),
    pubkey: "ab".repeat(32),
    created_at: 10,
    kind: 9,
    tags: [["h", "room"]],
    content: "hi",
    sig: "",
  };
  stores.scope(SCOPE).insertEvents([event]);
  stores
    .scope(SCOPE)
    .claimRoom("room", "top", { lo: null, loIsStart: true, hi: { at: 20 } });
  assert.equal(stores.scope(SCOPE).readWindow("room", null, 10).rows.length, 1);
  assert.equal(stores.scope(other).readWindow("room", null, 10).rows.length, 0);
  stores.clear(SCOPE);
  assert.equal(stores.scope(SCOPE).readWindow("room", null, 10).rows.length, 0);
});
