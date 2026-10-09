import assert from "node:assert/strict";
import test from "node:test";

import { isBrowserReloadKey, isBrowserUiKey } from "./useReloadShortcut.ts";

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

test("find, print and caret browsing keys are browser UI keys", () => {
  assert.equal(isBrowserUiKey(key({ key: "f" })), true);
  assert.equal(isBrowserUiKey(key({ key: "g", shiftKey: true })), true);
  assert.equal(isBrowserUiKey(key({ key: "p" })), true);
  assert.equal(isBrowserUiKey(key({ key: "F3", ctrlKey: false })), true);
  assert.equal(isBrowserUiKey(key({ key: "F7", ctrlKey: false })), true);
  assert.equal(isBrowserUiKey(key({ key: "k" })), false);
  assert.equal(isBrowserUiKey(key({ key: "f", ctrlKey: false })), false);
  assert.equal(isBrowserUiKey(key({ key: "f", altKey: true })), false);
  assert.equal(isBrowserUiKey(key()), false);
});
