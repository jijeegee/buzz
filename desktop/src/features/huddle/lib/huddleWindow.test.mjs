import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

import { huddleWindowChannelId } from "./huddleWindow.ts";

// The huddle window owns its own presence REQ (docs/message-sync.md), so the
// branch that recognises it must be reachable from a real window label.

const CHANNEL_ID = "0f8fad5b-d9cb-469f-a165-70867728950e";

function setWindowLabel(label) {
  globalThis.isTauri = true;
  globalThis.window = globalThis;
  globalThis.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label } },
  };
}

afterEach(() => {
  delete globalThis.isTauri;
  delete globalThis.__TAURI_INTERNALS__;
  delete globalThis.window;
});

test("a huddle window label yields its channel id", () => {
  setWindowLabel(`huddle-${CHANNEL_ID}`);
  assert.equal(huddleWindowChannelId(), CHANNEL_ID);
});

test("the main window is not a huddle window", () => {
  setWindowLabel("main");
  assert.equal(huddleWindowChannelId(), null);
});

test("a huddle label without a channel id is not a huddle window", () => {
  setWindowLabel("huddle-not-a-uuid");
  assert.equal(huddleWindowChannelId(), null);
});

test("outside Tauri there is no huddle window", () => {
  assert.equal(huddleWindowChannelId(), null);
});

test("Tauri without window metadata is not a huddle window", () => {
  globalThis.isTauri = true;
  globalThis.window = globalThis;
  globalThis.__TAURI_INTERNALS__ = {};
  assert.equal(huddleWindowChannelId(), null);
});
