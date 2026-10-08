import assert from "node:assert/strict";
import test from "node:test";

import { getInboxRoomEntry } from "./inboxRoomEntry.ts";

const ROOT = "a".repeat(64);

function row({
  channelId = "chan-1",
  channelType = "stream",
  conversationId = "evt-1",
  id = "evt-1",
  kind = 9,
  tags = [],
} = {}) {
  const item = { channelId, channelType, id, kind, pubkey: "p", tags };
  return {
    conversationId,
    groupItems: [item],
    id,
    isActionRequired: false,
    item,
  };
}

test("a thread reply lands on itself so its thread opens", () => {
  const entry = getInboxRoomEntry(
    row({ id: "reply", tags: [["e", ROOT, "", "reply"]] }),
    "all",
  );
  assert.deepEqual(entry, { channelId: "chan-1", messageId: "reply" });
});

test("a channel main row opens the room without a target", () => {
  const entry = getInboxRoomEntry(
    row({ conversationId: "channel:chan-1" }),
    "conversations",
  );
  assert.deepEqual(entry, { channelId: "chan-1", messageId: null });
});

test("a DM opens the room without a target in any filter", () => {
  const entry = getInboxRoomEntry(row({ channelType: "dm" }), "mention");
  assert.deepEqual(entry, { channelId: "chan-1", messageId: null });
});

test("a top-level mention outside the conversations view lands on it", () => {
  const entry = getInboxRoomEntry(row(), "mention");
  assert.deepEqual(entry, { channelId: "chan-1", messageId: "evt-1" });
});

test("rows without a channel have no chat room", () => {
  assert.equal(getInboxRoomEntry(row({ channelId: null }), "all"), null);
});

test("work that is not a chat message keeps the inbox detail", () => {
  assert.equal(getInboxRoomEntry(row(), "needs_action"), null);
  assert.equal(getInboxRoomEntry(row(), "agent_activity"), null);
  assert.equal(getInboxRoomEntry(row({ kind: 45001 }), "all"), null);
  assert.equal(
    getInboxRoomEntry({ ...row(), isActionRequired: true }, "all"),
    null,
  );
});
