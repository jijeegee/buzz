import assert from "node:assert/strict";
import test from "node:test";

import { getInboxRoomEntry } from "./inboxRoomEntry.ts";

const ROOT = "a".repeat(64);

function event({ id, createdAt = 10, kind = 9, tags = [] }) {
  return {
    channelId: "chan-1",
    channelType: "stream",
    createdAt,
    id,
    kind,
    pubkey: "p",
    tags,
  };
}

function row(
  groupItems,
  { channelType = "stream", isActionRequired = false } = {},
) {
  const item = { ...groupItems[0], channelType };
  return {
    conversationId: item.id,
    groupItems: groupItems.map((entry) => ({ ...entry, channelType })),
    id: item.id,
    isActionRequired,
    item,
  };
}

const reply = (id, createdAt) =>
  event({ createdAt, id, tags: [["e", ROOT, "", "reply"]] });

test("a thread row opens its thread at the latest reply", () => {
  const entry = getInboxRoomEntry(
    row([reply("older", 10), reply("newest", 30), reply("middle", 20)]),
    "conversations",
  );
  assert.deepEqual(entry, {
    channelId: "chan-1",
    messageId: "newest",
    opensThread: true,
    threadHeadId: ROOT,
  });
});

test("a channel main row lands on the room's newest message", () => {
  const entry = getInboxRoomEntry(
    row([
      event({ createdAt: 5, id: "old" }),
      event({ createdAt: 50, id: "new" }),
    ]),
    "conversations",
  );
  assert.deepEqual(entry, {
    channelId: "chan-1",
    messageId: null,
    opensThread: false,
    threadHeadId: null,
  });
});

test("a DM lands on the room's newest message in any chat filter", () => {
  const entry = getInboxRoomEntry(
    row([event({ id: "dm" })], { channelType: "dm" }),
    "mention",
  );
  assert.deepEqual(entry, {
    channelId: "chan-1",
    messageId: null,
    opensThread: false,
    threadHeadId: null,
  });
});

test("rows without a channel have no chat room", () => {
  const item = row([event({ id: "x" })]);
  item.item.channelId = null;
  assert.equal(getInboxRoomEntry(item, "all"), null);
});

test("work that is not a chat message keeps the inbox detail", () => {
  assert.equal(
    getInboxRoomEntry(row([event({ id: "a" })]), "needs_action"),
    null,
  );
  assert.equal(
    getInboxRoomEntry(row([event({ id: "a" })]), "agent_activity"),
    null,
  );
  assert.equal(
    getInboxRoomEntry(row([event({ id: "a", kind: 45001 })]), "all"),
    null,
  );
  assert.equal(
    getInboxRoomEntry(
      row([event({ id: "a" })], { isActionRequired: true }),
      "all",
    ),
    null,
  );
});

test("a single top-level mention lands on itself without a thread", () => {
  const entry = getInboxRoomEntry(row([event({ id: "mention" })]), "mention");
  assert.deepEqual(entry, {
    channelId: "chan-1",
    messageId: "mention",
    opensThread: false,
    threadHeadId: null,
  });
});
