import assert from "node:assert/strict";
import test from "node:test";

import { roomExitAction, threadRowMenuActions } from "./chatRowMenu.ts";

test("channels and group DMs are left, 1:1 DMs are hidden", () => {
  assert.equal(roomExitAction("stream", 0), "leave");
  assert.equal(roomExitAction("forum", 3), "leave");
  assert.equal(roomExitAction("dm", 1), "hide");
  // A DM with myself has no other participant.
  assert.equal(roomExitAction("dm", 0), "hide");
  assert.equal(roomExitAction("dm", 2), "leave");
});

test("thread row menu: pin, read toggle, unfollow, close toggle", () => {
  assert.deepEqual(
    threadRowMenuActions({
      hasUnread: false,
      isClosed: false,
      isPinned: false,
    }),
    ["pin", "mark-unread", "unfollow", "close"],
  );
  assert.deepEqual(
    threadRowMenuActions({ hasUnread: true, isClosed: true, isPinned: true }),
    ["unpin", "mark-read", "unfollow", "reopen"],
  );
});
