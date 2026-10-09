import assert from "node:assert/strict";
import test from "node:test";

import { isBrowserReloadKey } from "./useReloadShortcut.ts";

function key(overrides = {}) {
  return {
    altKey: false,
    ctrlKey: true,
    key: "r",
    metaKey: false,
    shiftKey: false,
    ...overrides,
  };
}

test("Ctrl+R, Ctrl+Shift+R and F5 are browser reload keys", () => {
  assert.equal(isBrowserReloadKey(key()), true);
  assert.equal(isBrowserReloadKey(key({ key: "R", shiftKey: true })), true);
  assert.equal(isBrowserReloadKey(key({ key: "F5", ctrlKey: false })), true);
  assert.equal(isBrowserReloadKey(key({ key: "F5" })), true);
});

test("other chords are not reload keys", () => {
  assert.equal(isBrowserReloadKey(key({ ctrlKey: false })), false);
  assert.equal(isBrowserReloadKey(key({ altKey: true })), false);
  assert.equal(isBrowserReloadKey(key({ key: "t" })), false);
  assert.equal(
    isBrowserReloadKey(key({ key: "F5", ctrlKey: false, altKey: true })),
    false,
  );
});
