import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  agentBadge,
  DEVICE_ROBOT_COLORS,
  DEVICE_ROBOT_SHAPES,
  deviceRobotTag,
  deviceRobotVariantForDevice,
  deviceRobotVariantFromTag,
} from "./deviceRobot.ts";
import { DEVICE_ROBOT_GEOMETRY } from "./deviceRobotGeometry.ts";

// Shared with mobile (device_robot_test.dart) and Tauri (device_robot.rs).
const GOLDEN = JSON.parse(
  readFileSync(
    new URL("../../../../test-fixtures/device-robots.json", import.meta.url),
    "utf8",
  ),
);

test("palette and silhouettes match the cross-platform fixture", () => {
  assert.deepEqual([...DEVICE_ROBOT_COLORS], GOLDEN.colors);
  assert.deepEqual([...DEVICE_ROBOT_SHAPES], GOLDEN.shapes);
  assert.deepEqual(
    JSON.parse(JSON.stringify(DEVICE_ROBOT_GEOMETRY)),
    GOLDEN.geometry,
  );
});

test("device ids map to the shared tags and variants", () => {
  for (const vector of GOLDEN.devices) {
    assert.equal(deviceRobotTag(vector.deviceId), vector.tag, vector.deviceId);
    const variant = deviceRobotVariantForDevice(vector.deviceId);
    assert.ok(variant, vector.deviceId);
    assert.equal(variant.tag, vector.tag);
    assert.equal(variant.colorIndex, vector.colorIndex, vector.deviceId);
    assert.equal(variant.shapeIndex, vector.shapeIndex, vector.deviceId);
    assert.equal(variant.color, GOLDEN.colors[vector.colorIndex]);
    assert.equal(variant.shape, GOLDEN.shapes[vector.shapeIndex]);
  }
});

test("tags map to the shared variants", () => {
  for (const vector of GOLDEN.extraTags) {
    const variant = deviceRobotVariantFromTag(vector.tag);
    assert.equal(variant?.colorIndex, vector.colorIndex, vector.tag);
    assert.equal(variant?.shapeIndex, vector.shapeIndex, vector.tag);
  }
});

test("unknown devices fall back to the default robot", () => {
  for (const id of [...GOLDEN.emptyDeviceIds, null, undefined]) {
    assert.equal(deviceRobotTag(id), null);
    assert.equal(deviceRobotVariantForDevice(id), null);
  }
  for (const tag of [...GOLDEN.invalidTags, null, undefined, 42]) {
    assert.equal(deviceRobotVariantFromTag(tag), null, String(tag));
  }
});

test("agent badge: device robot for the owner, owner mark for others", () => {
  const owner = "a".repeat(64);
  const hostDevice = GOLDEN.devices[0].tag;
  const mine = agentBadge({
    hostDevice,
    ownerPubkey: owner,
    viewerPubkey: owner.toUpperCase(),
  });
  assert.equal(mine.kind, "device");
  assert.equal(mine.variant.tag, hostDevice);
  assert.deepEqual(
    agentBadge({
      hostDevice,
      ownerPubkey: owner.toUpperCase(),
      viewerPubkey: "b".repeat(64),
    }),
    { kind: "owner", ownerPubkey: owner },
  );
  assert.deepEqual(
    agentBadge({
      hostDevice: null,
      ownerPubkey: owner,
      viewerPubkey: "b".repeat(64),
    }),
    { kind: "owner", ownerPubkey: owner },
    "others see the owner even without a host device",
  );
  for (const input of [
    { hostDevice, ownerPubkey: null, viewerPubkey: owner },
    { hostDevice: null, ownerPubkey: owner, viewerPubkey: owner },
    { hostDevice, ownerPubkey: owner, viewerPubkey: null },
  ]) {
    assert.deepEqual(agentBadge(input), { kind: "default" });
  }
});
