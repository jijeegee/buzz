import assert from "node:assert/strict";
import test from "node:test";

import {
  currentStorageKey,
  currentStorageValue,
  importPreRenameStorage,
} from "./preRenameStorage.ts";

function memoryStorage(entries) {
  const map = new Map(Object.entries(entries));
  return {
    get length() {
      return map.size;
    },
    key: (index) => [...map.keys()][index] ?? null,
    getItem: (key) => (map.has(key) ? map.get(key) : null),
    setItem: (key, value) => map.set(key, String(value)),
    removeItem: (key) => map.delete(key),
    map,
  };
}

test("maps keys and renamed values", () => {
  assert.equal(currentStorageKey("buzz-theme"), "kuzz-theme"); // kuzz-rename: keep
  assert.equal(currentStorageKey("buzz.appearance.Buzz"), "kuzz.appearance.Kuzz"); // kuzz-rename: keep
  assert.equal(currentStorageValue("buzz-dark"), "kuzz-dark"); // kuzz-rename: keep
  assert.equal(
    currentStorageValue('{"theme":"buzz","name":"buzzer"}'), // kuzz-rename: keep
    '{"theme":"kuzz","name":"buzzer"}', // kuzz-rename: keep
  );
});

test("copies once, keeps old keys, never overwrites current keys", () => {
  const storage = memoryStorage({
    "buzz-theme": "buzz", // kuzz-rename: keep
    "buzz:bot-recents": "[1]", // kuzz-rename: keep
    "kuzz:bot-recents": "[2]",
    unrelated: "x",
  });
  assert.equal(importPreRenameStorage(storage), 1);
  assert.equal(storage.getItem("kuzz-theme"), "kuzz");
  assert.equal(storage.getItem("kuzz:bot-recents"), "[2]");
  assert.equal(storage.getItem("buzz-theme"), "buzz"); // kuzz-rename: keep
  storage.removeItem("kuzz-theme");
  assert.equal(importPreRenameStorage(storage), 0);
  assert.equal(storage.getItem("kuzz-theme"), null);
});
