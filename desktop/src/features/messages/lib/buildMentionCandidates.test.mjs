import assert from "node:assert/strict";
import test from "node:test";

import { buildMentionCandidates } from "./buildMentionCandidates.ts";

const MEMBER_PUBKEY = "a".repeat(64);
const AGENT_PUBKEY = "b".repeat(64);
const ARCHIVED_PUBKEY = "c".repeat(64);
const SEARCHED_PUBKEY = "d".repeat(64);

function input(overrides = {}) {
  return {
    activeAgentPubkeys: new Set(),
    activePersonaById: new Map(),
    activePersonas: [],
    canSearchGlobalUsers: false,
    currentPubkey: null,
    isArchived: () => false,
    managedAgentDirectoryReady: true,
    managedAgentNamesByPubkey: new Map(),
    managedAgentPersonaIds: new Set(),
    managedAgentPersonaIdsByPubkey: new Map(),
    managedAgents: [],
    memberPubkeys: new Set(),
    members: [],
    mentionChannelId: null,
    mentionableAgentPubkeys: new Set(),
    personaNameByPubkey: new Map(),
    profiles: undefined,
    relayAgentDirectoryReady: true,
    relayAgentNamesByPubkey: new Map(),
    relayAgents: [],
    userSearchResults: [],
    ...overrides,
  };
}

test("a roster entry and its relay agent record coalesce into one candidate", () => {
  const candidates = buildMentionCandidates(
    input({
      members: [
        { pubkey: AGENT_PUBKEY, displayName: null, isAgent: true, role: "bot" },
      ],
      mentionableAgentPubkeys: new Set([AGENT_PUBKEY]),
      relayAgents: [
        {
          pubkey: AGENT_PUBKEY,
          name: "Scout",
          ownerPubkey: null,
          status: "online",
        },
      ],
    }),
  );

  assert.equal(candidates.length, 1);
  assert.equal(candidates[0].pubkey, AGENT_PUBKEY);
  // The roster contributes membership, the directory contributes the name.
  assert.equal(candidates[0].isMember, true);
  assert.equal(candidates[0].displayName, "Scout");
  assert.equal(candidates[0].isActiveAgent, true);
});

test("archived identities never become candidates", () => {
  const candidates = buildMentionCandidates(
    input({
      isArchived: (pubkey) => pubkey === ARCHIVED_PUBKEY,
      members: [
        { pubkey: MEMBER_PUBKEY, displayName: "Ada", isAgent: false },
        { pubkey: ARCHIVED_PUBKEY, displayName: "Gone", isAgent: false },
      ],
    }),
  );

  assert.deepEqual(
    candidates.map((candidate) => candidate.pubkey),
    [MEMBER_PUBKEY],
  );
});

const CHANNEL_ID = "room";

test("an agent outside the mentionable set is hidden once its directory is ready", () => {
  const relayAgents = [
    {
      pubkey: AGENT_PUBKEY,
      name: "Scout",
      ownerPubkey: null,
      status: "away",
      channelIds: [CHANNEL_ID],
    },
  ];

  assert.deepEqual(
    buildMentionCandidates(
      input({ mentionChannelId: CHANNEL_ID, relayAgents }),
    ),
    [],
  );
  assert.equal(
    buildMentionCandidates(
      input({
        mentionChannelId: CHANNEL_ID,
        mentionableAgentPubkeys: new Set([AGENT_PUBKEY]),
        relayAgents,
      }),
    ).length,
    1,
  );
});

test("only room members are offered: outside agents, personas and searched people never are", () => {
  const candidates = buildMentionCandidates(
    input({
      activePersonas: [
        {
          id: "planner",
          displayName: "Planner",
          avatarUrl: null,
          isActive: true,
        },
      ],
      canSearchGlobalUsers: true,
      currentPubkey: MEMBER_PUBKEY,
      managedAgentNamesByPubkey: new Map([[ARCHIVED_PUBKEY, "Mine"]]),
      managedAgents: [
        { pubkey: ARCHIVED_PUBKEY, name: "Mine", status: "running" },
      ],
      members: [{ pubkey: MEMBER_PUBKEY, displayName: "Ada", isAgent: false }],
      mentionChannelId: CHANNEL_ID,
      mentionableAgentPubkeys: new Set([AGENT_PUBKEY, ARCHIVED_PUBKEY]),
      relayAgents: [
        {
          pubkey: AGENT_PUBKEY,
          name: "Scout",
          ownerPubkey: MEMBER_PUBKEY,
          status: "online",
          channelIds: ["elsewhere"],
        },
      ],
      userSearchResults: [
        {
          pubkey: SEARCHED_PUBKEY,
          displayName: "Dana",
          isAgent: false,
          nip05Handle: null,
          ownerPubkey: null,
        },
      ],
    }),
  );

  assert.deepEqual(
    candidates.map((candidate) => candidate.pubkey),
    [MEMBER_PUBKEY],
  );
});

test("a directory agent already in this room counts as a member", () => {
  const [candidate] = buildMentionCandidates(
    input({
      mentionChannelId: CHANNEL_ID,
      mentionableAgentPubkeys: new Set([AGENT_PUBKEY]),
      relayAgents: [
        {
          pubkey: AGENT_PUBKEY,
          name: "Scout",
          ownerPubkey: MEMBER_PUBKEY,
          status: "unknown",
          channelIds: [CHANNEL_ID],
        },
      ],
    }),
  );
  assert.equal(candidate.pubkey, AGENT_PUBKEY);
  assert.equal(candidate.isMember, true);
  // Policy-only discovery stays selectable without claiming active presence.
  assert.equal(candidate.isActiveAgent, false);
  assert.equal(candidate.ownerPubkey, MEMBER_PUBKEY);
});

for (const locallyManaged of [true, false]) {
  test(`roster candidate preserves exact local management: ${locallyManaged}`, () => {
    const [candidate] = buildMentionCandidates(
      input({
        members: [{ pubkey: AGENT_PUBKEY, displayName: "Scout", role: "bot" }],
        managedAgentNamesByPubkey: new Map(
          locallyManaged ? [[AGENT_PUBKEY, "Scout"]] : [],
        ),
        managedAgents: locallyManaged
          ? [{ pubkey: AGENT_PUBKEY, name: "Scout", status: "deployed" }]
          : [],
        mentionableAgentPubkeys: new Set([AGENT_PUBKEY]),
      }),
    );
    assert.equal(candidate.isMember, true);
    assert.equal(Boolean(candidate.isManagedAgent), locallyManaged);
  });
}
