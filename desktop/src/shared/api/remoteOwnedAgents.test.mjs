import assert from "node:assert/strict";
import test from "node:test";

import {
  findAgentNameConflict,
  agentNameTakenMessage,
} from "./agentNameConflict.ts";
import {
  buildRemoteOwnedAgents,
  remoteAgentPersonaId,
} from "./remoteOwnedAgents.ts";

const OWNER = "a".repeat(64);
const OTHER = "b".repeat(64);
const AGENT_1 = "1".repeat(64);
const AGENT_2 = "2".repeat(64);
const AGENT_3 = "3".repeat(64);
const THIS_PC = "device-this";
const PC_A = "device-a";

let nextId = 0;
function projection({
  agent,
  name,
  personaId,
  createdAt = 100,
  pubkey = OWNER,
}) {
  nextId += 1;
  return {
    id: String(nextId).padStart(64, "0"),
    kind: 30177,
    pubkey,
    created_at: createdAt,
    tags: [["d", agent]],
    content: JSON.stringify({
      name,
      ...(personaId ? { persona_id: personaId } : {}),
      parallelism: 1,
      respond_to: "owner-only",
    }),
    sig: "",
  };
}

test("agents hosted on another device and not local are remote", () => {
  const agents = buildRemoteOwnedAgents({
    hostDevices: new Map([
      [AGENT_1, PC_A],
      [AGENT_2, THIS_PC],
      [AGENT_3, PC_A],
    ]),
    currentDeviceIds: [THIS_PC],
    localPubkeys: [AGENT_3.toUpperCase()],
    projections: [
      projection({ agent: AGENT_1, name: "Fizz", personaId: "p1" }),
    ],
    ownerPubkey: OWNER,
  });
  assert.deepEqual(agents, [
    { pubkey: AGENT_1, deviceId: PC_A, name: "Fizz", personaId: "p1" },
  ]);
});

test("the newest projection by the owner names the agent", () => {
  const [agent] = buildRemoteOwnedAgents({
    hostDevices: new Map([[AGENT_1, PC_A]]),
    currentDeviceIds: [],
    localPubkeys: [],
    projections: [
      projection({ agent: AGENT_1, name: "Old", createdAt: 100 }),
      projection({ agent: AGENT_1, name: "New", createdAt: 200 }),
      projection({
        agent: AGENT_1,
        name: "Forged",
        createdAt: 300,
        pubkey: OTHER,
      }),
      {
        ...projection({ agent: AGENT_1, name: "x", createdAt: 400 }),
        content: "{",
      },
    ],
    ownerPubkey: OWNER,
  });
  assert.equal(agent.name, "New");
  assert.equal(agent.personaId, null);
});

test("an agent without a projection yet is still listed, unnamed", () => {
  const [agent] = buildRemoteOwnedAgents({
    hostDevices: new Map([[AGENT_1, PC_A]]),
    currentDeviceIds: [],
    localPubkeys: [],
    projections: [],
    ownerPubkey: OWNER,
  });
  assert.deepEqual(agent, {
    pubkey: AGENT_1,
    deviceId: PC_A,
    name: null,
    personaId: null,
  });
});

test("persona match prefers the id, then a unique name", () => {
  const personas = [
    { id: "p1", displayName: "Fizz" },
    { id: "slug-honey", displayName: "Honey" },
    { id: "d1", displayName: "Twin" },
    { id: "d2", displayName: "twin" },
  ];
  assert.equal(
    remoteAgentPersonaId({ name: "Other", personaId: "p1" }, personas),
    "p1",
  );
  // A team persona has another local id on the hosting computer.
  assert.equal(
    remoteAgentPersonaId(
      { name: " honey ", personaId: "uuid-on-pc-a" },
      personas,
    ),
    "slug-honey",
  );
  assert.equal(
    remoteAgentPersonaId({ name: "Twin", personaId: null }, personas),
    null,
  );
  assert.equal(
    remoteAgentPersonaId({ name: null, personaId: "nope" }, personas),
    null,
  );
});

const ROSTER = {
  agents: [
    { pubkey: AGENT_1, name: "Fizz", personaId: "p-fizz" },
    { pubkey: AGENT_2, name: "Scout", personaId: null },
  ],
  personas: [
    { id: "p-fizz", displayName: "Fizz" },
    { id: "p-honey", displayName: "Honey" },
  ],
};

test("a new agent may not reuse any agent or persona name", () => {
  assert.equal(
    findAgentNameConflict({ kind: "agent", name: " fizz " }, ROSTER),
    "Fizz",
  );
  // A second agent from the same persona is still a second "Fizz".
  assert.equal(
    findAgentNameConflict(
      { kind: "agent", name: "Fizz", personaId: "p-fizz" },
      ROSTER,
    ),
    "Fizz",
  );
  assert.equal(
    findAgentNameConflict({ kind: "agent", name: "Honey" }, ROSTER),
    "Honey",
  );
  // The first agent of a persona shares the persona's name.
  assert.equal(
    findAgentNameConflict(
      { kind: "agent", name: "Honey", personaId: "p-honey" },
      ROSTER,
    ),
    null,
  );
  assert.equal(
    findAgentNameConflict({ kind: "agent", name: "Pollen" }, ROSTER),
    null,
  );
  assert.equal(
    findAgentNameConflict({ kind: "agent", name: "  " }, ROSTER),
    null,
  );
});

test("renaming an agent ignores the agent itself", () => {
  assert.equal(
    findAgentNameConflict(
      { kind: "agent", name: "SCOUT", selfPubkey: AGENT_2.toUpperCase() },
      ROSTER,
    ),
    null,
  );
  assert.equal(
    findAgentNameConflict(
      { kind: "agent", name: "Fizz", selfPubkey: AGENT_2 },
      ROSTER,
    ),
    "Fizz",
  );
});

test("a persona may not take another agent's or persona's name", () => {
  assert.equal(
    findAgentNameConflict({ kind: "persona", name: "Scout" }, ROSTER),
    "Scout",
  );
  assert.equal(
    findAgentNameConflict({ kind: "persona", name: "honey" }, ROSTER),
    "Honey",
  );
  // Its own agents keep it company.
  assert.equal(
    findAgentNameConflict(
      { kind: "persona", name: "Fizz", selfId: "p-fizz" },
      ROSTER,
    ),
    null,
  );
  assert.equal(
    findAgentNameConflict(
      { kind: "persona", name: "Fizz", selfId: "p-honey" },
      ROSTER,
    ),
    "Fizz",
  );
});

test("the message names the clash", () => {
  assert.match(agentNameTakenMessage(" Fizz "), /named "Fizz"/);
});
