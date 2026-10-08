import assert from "node:assert/strict";
import test from "node:test";

import {
  formatInboxConversationRoomTitle,
  resolveInboxConversationRoom,
} from "./inboxConversationRoom.ts";

const ME = "a".repeat(64);
const ALICE = "b".repeat(64);

function inboxItem({ channelType = "stream", tags = [], pubkey = ME } = {}) {
  return {
    avatarUrl: "https://example.com/me.png",
    channelLabel: "kuzz",
    groupItems: [],
    item: {
      channelId: "chan-1",
      channelType,
      id: "evt-1",
      kind: 9,
      pubkey,
      tags,
    },
    senderLabel: "Me",
  };
}

const noAgents = () => false;

test("a DM room is shown as the other participant even when I sent last", () => {
  const room = resolveInboxConversationRoom(inboxItem({ channelType: "dm" }), {
    dmChannelLabels: { "chan-1": "Alice" },
    dmParticipantsByChannelId: {
      "chan-1": [
        {
          avatarUrl: "https://example.com/alice.png",
          label: "Alice",
          pubkey: ALICE,
        },
      ],
    },
    isAgentPubkey: noAgents,
  });
  assert.equal(room.kind, "dm");
  assert.equal(room.person.pubkey, ALICE);
  assert.equal(room.person.avatarUrl, "https://example.com/alice.png");
  assert.equal(formatInboxConversationRoomTitle(room), "Alice");
});

test("a DM room marks an agent counterpart as an agent", () => {
  const room = resolveInboxConversationRoom(inboxItem({ channelType: "dm" }), {
    dmParticipantsByChannelId: {
      "chan-1": [{ avatarUrl: null, label: "Fizz", pubkey: ALICE }],
    },
    isAgentPubkey: (pubkey) => pubkey === ALICE,
  });
  assert.equal(room.kind, "dm");
  assert.equal(room.person.isAgent, true);
  assert.equal(formatInboxConversationRoomTitle(room), "Fizz");
});

test("a channel main message is the channel room", () => {
  const room = resolveInboxConversationRoom(inboxItem(), {
    isAgentPubkey: noAgents,
  });
  assert.deepEqual(room, {
    kind: "channel",
    channelId: "chan-1",
    channelLabel: "kuzz",
  });
  assert.equal(formatInboxConversationRoomTitle(room), "#kuzz");
});

test("a thread reply is the thread room named after the thread", () => {
  const root = "c".repeat(64);
  const room = resolveInboxConversationRoom(
    inboxItem({ tags: [["e", root, "", "reply"]] }),
    { isAgentPubkey: noAgents },
  );
  assert.equal(room.kind, "thread");
  assert.equal(room.threadId, root);
  assert.equal(
    formatInboxConversationRoomTitle(room, "Activity filter"),
    "#kuzz › Activity filter",
  );
  assert.equal(formatInboxConversationRoomTitle(room, null), "#kuzz › Thread");
});
