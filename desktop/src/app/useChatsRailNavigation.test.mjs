import assert from "node:assert/strict";
import test from "node:test";

import { resolveChatsRailTarget } from "./useChatsRailNavigation.ts";

test("Chats rail returns to the last open room while it is still listed", () => {
  assert.equal(resolveChatsRailTarget("b", ["a", "b"]), "b");
});

test("Chats rail falls back to the first sidebar chat", () => {
  assert.equal(resolveChatsRailTarget("gone", ["a", "b"]), "a");
  assert.equal(resolveChatsRailTarget(null, ["a"]), "a");
  assert.equal(resolveChatsRailTarget(null, []), null);
});
