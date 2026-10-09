import assert from "node:assert/strict";
import test from "node:test";

import {
  buildAgentHostDeviceMap,
  buildDeviceRobotOverrides,
  buildOwnerDevices,
  parseAgentHostDevicesContent,
} from "./ownerDevices.ts";

const OWNER = "a".repeat(64);
const AGENT_1 = "1".repeat(64);
const AGENT_2 = "2".repeat(64);
const AGENT_3 = "3".repeat(64);
const DEVICE_A = "0b9e3c2a-1111-4c4c-8888-aaaaaaaaaaaa";
const DEVICE_B = "0b9e3c2a-2222-4c4c-8888-bbbbbbbbbbbb";

let nextId = 0;
function hostEvent({
  createdAt,
  deviceId,
  agents,
  content,
  id,
  kind = 30180,
  pubkey = OWNER,
  tags,
}) {
  nextId += 1;
  return {
    id: id ?? nextId.toString(16).padStart(64, "0"),
    pubkey,
    created_at: createdAt,
    kind,
    tags: tags ?? [["d", deviceId]],
    content: content ?? JSON.stringify({ v: 1, agents }),
    sig: "",
  };
}

test("maps each listed agent to the device that published it", () => {
  const map = buildAgentHostDeviceMap(
    [
      hostEvent({ createdAt: 10, deviceId: DEVICE_A, agents: [AGENT_1] }),
      hostEvent({
        createdAt: 20,
        deviceId: DEVICE_B,
        agents: [AGENT_2, AGENT_3.toUpperCase()],
      }),
    ],
    OWNER,
  );
  assert.deepEqual(
    [...map.entries()].sort(),
    [
      [AGENT_1, DEVICE_A],
      [AGENT_2, DEVICE_B],
      [AGENT_3, DEVICE_B],
    ].sort(),
  );
});

test("an agent listed by several devices goes to the newest event", () => {
  const events = [
    hostEvent({ createdAt: 10, deviceId: DEVICE_A, agents: [AGENT_1] }),
    hostEvent({ createdAt: 30, deviceId: DEVICE_B, agents: [AGENT_1] }),
  ];
  assert.equal(buildAgentHostDeviceMap(events, OWNER).get(AGENT_1), DEVICE_B);
  assert.equal(
    buildAgentHostDeviceMap([...events].reverse(), OWNER).get(AGENT_1),
    DEVICE_B,
    "relay order does not matter",
  );
});

test("only a device's newest revision counts", () => {
  const map = buildAgentHostDeviceMap(
    [
      hostEvent({ createdAt: 10, deviceId: DEVICE_A, agents: [AGENT_1] }),
      hostEvent({ createdAt: 20, deviceId: DEVICE_A, agents: [AGENT_2] }),
    ],
    OWNER,
  );
  assert.equal(map.get(AGENT_1), undefined, "dropped agent is not resurrected");
  assert.equal(map.get(AGENT_2), DEVICE_A);
});

test("device ids are normalised to lowercase", () => {
  const map = buildAgentHostDeviceMap(
    [
      hostEvent({
        createdAt: 10,
        deviceId: ` ${DEVICE_A.toUpperCase()} `,
        agents: [AGENT_1],
      }),
    ],
    OWNER,
  );
  assert.equal(map.get(AGENT_1), DEVICE_A);
});

test("malformed events and foreign authors are ignored", () => {
  const valid = hostEvent({
    createdAt: 10,
    deviceId: DEVICE_A,
    agents: [AGENT_1],
  });
  const map = buildAgentHostDeviceMap(
    [
      valid,
      // Newer, but malformed: none of these may claim AGENT_1 or AGENT_2.
      hostEvent({ createdAt: 50, deviceId: DEVICE_B, content: "not json" }),
      hostEvent({
        createdAt: 50,
        deviceId: DEVICE_B,
        content: JSON.stringify({ v: 2, agents: [AGENT_1] }),
      }),
      hostEvent({
        createdAt: 50,
        deviceId: DEVICE_B,
        content: JSON.stringify({ v: 1, agents: AGENT_1 }),
      }),
      hostEvent({ createdAt: 50, tags: [], agents: [AGENT_1] }),
      hostEvent({ createdAt: 50, tags: [["d", "  "]], agents: [AGENT_1] }),
      hostEvent({
        createdAt: 50,
        deviceId: DEVICE_B,
        agents: [AGENT_1],
        pubkey: "b".repeat(64),
      }),
      hostEvent({
        createdAt: 50,
        deviceId: DEVICE_B,
        agents: [AGENT_1],
        kind: 30078,
      }),
      // A malformed newer head for DEVICE_A leaves the valid one standing.
      hostEvent({ createdAt: 60, deviceId: DEVICE_A, content: "[]" }),
      hostEvent({
        createdAt: 70,
        deviceId: DEVICE_B,
        agents: ["not-a-pubkey", 42, AGENT_2],
      }),
    ],
    OWNER.toUpperCase(),
  );
  assert.equal(map.get(AGENT_1), DEVICE_A);
  assert.equal(map.get(AGENT_2), DEVICE_B, "valid entries beside junk survive");
  assert.equal(map.size, 2);
});

test("content parser accepts only v1 agent lists", () => {
  assert.deepEqual(
    parseAgentHostDevicesContent(JSON.stringify({ v: 1, agents: [] })),
    [],
  );
  for (const content of ["", "null", "1", "[]", '{"v":1}', '{"agents":[]}']) {
    assert.equal(parseAgentHostDevicesContent(content), null, content);
  }
});

function robotEvent({ createdAt, deviceId, shape, color, content, pubkey }) {
  return hostEvent({
    createdAt,
    deviceId,
    kind: 30181,
    pubkey,
    content: content ?? JSON.stringify({ v: 1, shape, color }),
  });
}

test("robot overrides: newest valid choice per device wins", () => {
  const overrides = buildDeviceRobotOverrides(
    [
      robotEvent({
        createdAt: 10,
        deviceId: DEVICE_A,
        shape: "dome",
        color: 1,
      }),
      robotEvent({ createdAt: 20, deviceId: DEVICE_A, shape: "hex", color: 7 }),
      robotEvent({ createdAt: 5, deviceId: DEVICE_B, shape: "tall", color: 0 }),
    ],
    OWNER,
  );
  assert.deepEqual(overrides.get(DEVICE_A), {
    shape: "hex",
    colorIndex: 7,
    createdAt: 20,
  });
  assert.deepEqual(overrides.get(DEVICE_B), {
    shape: "tall",
    colorIndex: 0,
    createdAt: 5,
  });
});

test("robot overrides: unknown shapes, bad colours and strangers are ignored", () => {
  const overrides = buildDeviceRobotOverrides(
    [
      robotEvent({
        createdAt: 10,
        deviceId: DEVICE_A,
        shape: "dome",
        color: 2,
      }),
      robotEvent({
        createdAt: 11,
        deviceId: DEVICE_A,
        shape: "blob",
        color: 2,
      }),
      robotEvent({ createdAt: 12, deviceId: DEVICE_A, shape: "hex", color: 8 }),
      robotEvent({
        createdAt: 13,
        deviceId: DEVICE_A,
        shape: "hex",
        color: -1,
      }),
      robotEvent({
        createdAt: 14,
        deviceId: DEVICE_A,
        shape: "hex",
        color: 1.5,
      }),
      robotEvent({
        createdAt: 15,
        deviceId: DEVICE_A,
        shape: "hex",
        color: "1",
      }),
      robotEvent({ createdAt: 16, deviceId: DEVICE_A, content: "{" }),
      robotEvent({
        createdAt: 17,
        deviceId: DEVICE_A,
        content: JSON.stringify({ v: 2, shape: "hex", color: 1 }),
      }),
      robotEvent({
        createdAt: 18,
        deviceId: DEVICE_A,
        shape: "hex",
        color: 1,
        pubkey: "b".repeat(64),
      }),
      robotEvent({
        createdAt: 19,
        deviceId: DEVICE_B,
        shape: "blob",
        color: 9,
      }),
    ],
    OWNER,
  );
  assert.deepEqual(overrides.get(DEVICE_A), {
    shape: "dome",
    colorIndex: 2,
    createdAt: 10,
  });
  assert.equal(overrides.has(DEVICE_B), false);
});

test("one fetch carries both host devices and robot choices", () => {
  const owner = buildOwnerDevices(
    [
      hostEvent({ createdAt: 10, deviceId: DEVICE_A, agents: [AGENT_1] }),
      robotEvent({
        createdAt: 10,
        deviceId: DEVICE_A,
        shape: "boxy",
        color: 3,
      }),
    ],
    OWNER,
  );
  assert.equal(owner.hostDevices.get(AGENT_1), DEVICE_A);
  assert.equal(owner.robotOverrides.get(DEVICE_A)?.shape, "boxy");
  assert.equal(owner.hostDevices.size, 1, "a 30181 lists no agents");
});
