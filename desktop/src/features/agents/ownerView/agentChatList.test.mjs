import assert from "node:assert/strict";
import test from "node:test";

import {
  agentChatChannels,
  channelFromAgentChatRow,
  loadAgentChatList,
  ownerViewChatRow,
  withSeenDots,
} from "./agentChatList.ts";

const AGENT = "a".repeat(64);
const PERSON = "b".repeat(64);
const OTHER = "c".repeat(64);

function row(overrides) {
  return {
    type: "room",
    key: "chat-1",
    at: 100,
    at_us: 100_000_000,
    channel_id: "chat-1",
    channel_type: "dm",
    channel_name: "",
    archived: false,
    dm_members: [PERSON],
    root_id: null,
    thread_name: null,
    follow_state: null,
    closed: false,
    last_seq: null,
    read_seq: null,
    unread: 0,
    unread_overflow: false,
    unread_mentions: 0,
    unread_mentions_overflow: false,
    unread_mark: false,
    latest: {
      id: "e1",
      pubkey: PERSON,
      created_at: 100,
      kind: 9,
      tags: [],
      content: "hi",
    },
    root: null,
    ...overrides,
  };
}

test("a DM row becomes a read-only DM with the agent first", () => {
  const channel = channelFromAgentChatRow(row({}), AGENT.toUpperCase());
  assert.equal(channel.channelType, "dm");
  assert.equal(channel.isMember, false);
  assert.equal(channel.visibility, "private");
  assert.deepEqual(channel.participantPubkeys, [AGENT, PERSON]);
  assert.equal(channel.lastMessageAt, new Date(100_000).toISOString());
});

test("a group chat row keeps its flag and member count", () => {
  const channel = channelFromAgentChatRow(
    row({
      channel_type: "stream",
      group_chat: true,
      member_count: 3,
      dm_members: [PERSON, OTHER],
    }),
    AGENT,
  );
  assert.equal(channel.channelType, "stream");
  assert.equal(channel.groupChat, true);
  assert.equal(channel.memberCount, 3);
  assert.deepEqual(channel.memberPubkeys, [AGENT, PERSON, OTHER]);
});

test("dots mark what moved since the owner last looked, never the agent's own", () => {
  const rows = [
    row({ key: "new", channel_id: "new" }),
    row({ key: "seen", channel_id: "seen" }),
    row({
      key: "mine",
      channel_id: "mine",
      latest: { ...row({}).latest, pubkey: AGENT },
    }),
  ];
  const seenAt = (contextId) => (contextId === "seen" ? 100 : 50);
  const dotted = withSeenDots(rows, AGENT, seenAt);
  assert.deepEqual(
    dotted.map((r) => [r.key, r.unread_mark, r.unread]),
    [
      ["new", true, 0],
      ["seen", false, 0],
      ["mine", false, 0],
    ],
  );
});

test("thread rows use their own seen position", () => {
  const thread = row({ type: "thread", key: "root-1", root_id: "root-1" });
  const contexts = [];
  withSeenDots([thread], AGENT, (contextId) => {
    contexts.push(contextId);
    return 0;
  });
  assert.deepEqual(contexts, ["thread:root-1"]);
});

test("pages load until the end or the row cap, without duplicates", async () => {
  const pages = [
    {
      rows: [row({ key: "1" }), row({ key: "2" })],
      next: { at_us: 2, key: "2" },
    },
    {
      rows: [row({ key: "2" }), row({ key: "3" })],
      next: { at_us: 3, key: "3" },
    },
    { rows: [row({ key: "4" })], next: null },
  ];
  const cursors = [];
  const all = await loadAgentChatList(async (before) => {
    cursors.push(before?.key ?? null);
    return pages[cursors.length - 1];
  });
  assert.deepEqual(
    all.map((r) => r.key),
    ["1", "2", "3", "4"],
  );
  assert.deepEqual(cursors, [null, "2", "3"]);

  let calls = 0;
  const capped = await loadAgentChatList(async () => {
    calls += 1;
    return pages[calls - 1];
  }, 2);
  assert.equal(capped.length, 2);
  assert.equal(calls, 1);
});

test("a chat is found by its room row, else by a thread row", () => {
  const threadOnly = row({
    type: "thread",
    key: "root-9",
    root_id: "root-9",
    channel_id: "chat-9",
  });
  const rows = [
    row({ type: "thread", key: "root-1", root_id: "root-1" }),
    row({}),
    threadOnly,
  ];
  assert.equal(ownerViewChatRow(rows, "chat-1")?.type, "room");
  assert.equal(ownerViewChatRow(rows, "chat-9"), threadOnly);
  assert.equal(ownerViewChatRow(rows, "missing"), null);
});

test("a DM listed by its thread row first still names the other person", () => {
  const thread = row({
    type: "thread",
    key: "root-1",
    root_id: "root-1",
    dm_members: [],
  });
  const room = row({ at: 90 });
  const [channel] = agentChatChannels([thread, room], AGENT);
  assert.deepEqual(channel.participantPubkeys, [AGENT, PERSON]);
  assert.equal(channel.isMember, true);
  assert.equal(agentChatChannels([room, thread], AGENT).length, 1);
});
