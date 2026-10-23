import assert from "node:assert/strict";
import test from "node:test";

import { ownerReaders } from "./OwnerCanReadNotice.tsx";

const ME = "1".repeat(64);
const FRIEND = "2".repeat(64);
const AGENT = "3".repeat(64);
const OWNER = "4".repeat(64);

function channel(overrides) {
  return {
    id: "c",
    name: "",
    channelType: "dm",
    visibility: "private",
    description: "",
    topic: null,
    purpose: null,
    memberCount: 2,
    memberPubkeys: [],
    lastMessageAt: null,
    archivedAt: null,
    participants: [],
    participantPubkeys: [],
    isMember: true,
    ttlSeconds: null,
    ttlDeadline: null,
    goalsEnabled: true,
    ...overrides,
  };
}

function member(pubkey, isAgent = false) {
  return {
    pubkey,
    role: isAgent ? "bot" : "member",
    isAgent,
    joinedAt: "",
    displayName: null,
  };
}

const profiles = {
  [AGENT]: {
    displayName: "Fizz",
    avatarUrl: null,
    nip05Handle: null,
    ownerPubkey: OWNER,
    isAgent: true,
  },
};

test("the owner of an agent in my DM can read it", () => {
  assert.deepEqual(
    ownerReaders(channel({}), [member(ME), member(AGENT, true)], profiles, ME),
    [{ agentPubkey: AGENT, ownerPubkey: OWNER }],
  );
});

test("no notice when the owner is in the chat, or is me", () => {
  const members = [member(ME), member(AGENT, true), member(OWNER)];
  assert.deepEqual(ownerReaders(channel({}), members, profiles, ME), []);
  assert.deepEqual(
    ownerReaders(
      channel({}),
      [member(OWNER), member(AGENT, true)],
      profiles,
      OWNER,
    ),
    [],
  );
});

test("only DMs and group chats", () => {
  const members = [member(ME), member(FRIEND), member(AGENT, true)];
  assert.equal(
    ownerReaders(
      channel({ groupChat: true, channelType: "stream" }),
      members,
      profiles,
      ME,
    ).length,
    1,
  );
  assert.deepEqual(
    ownerReaders(channel({ channelType: "stream" }), members, profiles, ME),
    [],
  );
});
