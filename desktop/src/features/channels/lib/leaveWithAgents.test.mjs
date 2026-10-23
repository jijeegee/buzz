import assert from "node:assert/strict";
import test from "node:test";

import {
  isSoleOwner,
  leaveRemovingAgents,
  ownAgentsToRemoveOnLeave,
} from "./leaveWithAgents.ts";

const ME = "a".repeat(64);
const OTHER = "b".repeat(64);
const MY_AGENT = "c".repeat(64);
const MY_OTHER_AGENT = "d".repeat(64);
const THEIR_AGENT = "e".repeat(64);
const MANAGED = new Set([MY_AGENT, MY_OTHER_AGENT]);

function member(pubkey, role = "member") {
  return { displayName: null, isAgent: false, joinedAt: "", pubkey, role };
}

test("my agents in the room are offered for removal, others' are not", () => {
  const members = [
    member(ME),
    member(OTHER, "owner"),
    member(MY_AGENT.toUpperCase(), "bot"),
    member(THEIR_AGENT, "bot"),
  ];
  assert.deepEqual(ownAgentsToRemoveOnLeave(members, MANAGED, ME), [MY_AGENT]);
  assert.deepEqual(
    ownAgentsToRemoveOnLeave([member(ME), member(OTHER)], MANAGED, ME),
    [],
  );
  assert.deepEqual(ownAgentsToRemoveOnLeave(members, MANAGED, undefined), []);
});

test("the only owner gets no agent removal: that leave is refused", () => {
  const members = [member(ME, "owner"), member(OTHER), member(MY_AGENT, "bot")];
  assert.equal(isSoleOwner(members, ME), true);
  assert.deepEqual(ownAgentsToRemoveOnLeave(members, MANAGED, ME), []);

  const coOwned = [...members, member(OTHER.replace("b", "f"), "owner")];
  assert.equal(isSoleOwner(coOwned, ME), false);
  assert.deepEqual(ownAgentsToRemoveOnLeave(coOwned, MANAGED, ME), [MY_AGENT]);
});

test("agents are removed one by one before the leave", async () => {
  const calls = [];
  const result = await leaveRemovingAgents({
    agentPubkeys: [MY_AGENT, MY_OTHER_AGENT],
    leave: async () => {
      calls.push("leave");
    },
    removeMember: async (pubkey) => {
      calls.push(`remove:${pubkey}`);
    },
  });
  assert.deepEqual(calls, [
    `remove:${MY_AGENT}`,
    `remove:${MY_OTHER_AGENT}`,
    "leave",
  ]);
  assert.deepEqual(result.failedAgentRemovals, []);
});

test("an agent that is already gone does not stop the rest or the leave", async () => {
  const calls = [];
  const result = await leaveRemovingAgents({
    agentPubkeys: [MY_AGENT, MY_OTHER_AGENT],
    leave: async () => {
      calls.push("leave");
    },
    removeMember: async (pubkey) => {
      calls.push(`remove:${pubkey}`);
      if (pubkey === MY_AGENT) throw new Error("not a member");
    },
  });
  assert.deepEqual(calls, [
    `remove:${MY_AGENT}`,
    `remove:${MY_OTHER_AGENT}`,
    "leave",
  ]);
  assert.deepEqual(result.failedAgentRemovals, [MY_AGENT]);
});

test("leave only removes nothing, and a refused leave still fails", async () => {
  const calls = [];
  await assert.rejects(
    leaveRemovingAgents({
      agentPubkeys: [],
      leave: async () => {
        calls.push("leave");
        throw new Error("the last owner cannot leave");
      },
      removeMember: async (pubkey) => {
        calls.push(`remove:${pubkey}`);
      },
    }),
    /last owner/,
  );
  assert.deepEqual(calls, ["leave"]);
});
