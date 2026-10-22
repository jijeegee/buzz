import assert from "node:assert/strict";
import test from "node:test";

import {
  resolveChatsRailTarget,
  shouldLandOnChats,
} from "./useChatsRailNavigation.ts";

test("Chats rail returns to the last open room while it is still listed", () => {
  assert.equal(resolveChatsRailTarget("b", ["a", "b"]), "b");
});

test("Chats rail falls back to the first sidebar chat", () => {
  assert.equal(resolveChatsRailTarget("gone", ["a", "b"]), "a");
  assert.equal(resolveChatsRailTarget(null, ["a"]), "a");
  assert.equal(resolveChatsRailTarget(null, []), null);
});

test("home lands on Chats unless a welcome room is waiting", () => {
  const home = { selectedView: "home", pendingWelcome: false };
  assert.equal(shouldLandOnChats(home), true);
  // Already somewhere else, or a new account's welcome room opens instead.
  assert.equal(shouldLandOnChats({ ...home, selectedView: "channel" }), false);
  assert.equal(shouldLandOnChats({ ...home, pendingWelcome: true }), false);
});
