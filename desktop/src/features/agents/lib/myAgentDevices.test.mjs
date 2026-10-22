import assert from "node:assert/strict";
import test from "node:test";

import {
  groupRemoteAgentsByDevice,
  localAgentsForList,
  presetUsageLabel,
} from "./myAgentDevices.ts";

test("every live agent of a preset gets its own row, archived ones none", () => {
  const agents = [
    { name: "fizz 2", pubkey: "b".repeat(64), personaId: "p1" },
    { name: "Fizz", pubkey: "a".repeat(64), personaId: "p1" },
    { name: "Old", pubkey: "c".repeat(64), personaId: null },
  ];
  const rows = localAgentsForList(agents, (pubkey) => pubkey.startsWith("c"));
  assert.deepEqual(
    rows.map((agent) => agent.name),
    ["Fizz", "fizz 2"],
  );
  // The input order is left alone.
  assert.equal(agents[0].name, "fizz 2");
});

test("remote agents are grouped per computer, named computers first", () => {
  const names = new Map([
    ["dev-mac", "Mac mini"],
    ["dev-air", "air"],
  ]);
  const groups = groupRemoteAgentsByDevice(
    [
      { pubkey: "1", deviceId: "dev-mac", name: "Zed" },
      { pubkey: "2", deviceId: "dev-x", name: "Lost" },
      { pubkey: "3", deviceId: "dev-mac", name: "astra" },
      { pubkey: "4", deviceId: "dev-air", name: "Bee" },
    ],
    (deviceId) => names.get(deviceId),
    (agent) => agent.name,
  );
  assert.deepEqual(
    groups.map((group) => [group.name, group.agents.map((a) => a.name)]),
    [
      ["air", ["Bee"]],
      ["Mac mini", ["astra", "Zed"]],
      ["", ["Lost"]],
    ],
  );
});

test("preset usage reads like the mockup", () => {
  assert.equal(presetUsageLabel(0), "not used yet");
  assert.equal(presetUsageLabel(1), "used by 1 agent");
  assert.equal(presetUsageLabel(3), "used by 3 agents");
});
