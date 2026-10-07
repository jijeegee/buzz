import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  agentDeviceRobotVariant,
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

test("only the agent's verified owner sees the device robot", () => {
  const owner = "a".repeat(64);
  const hostDevice = GOLDEN.devices[0].tag;
  assert.equal(
    agentDeviceRobotVariant({
      hostDevice,
      ownerPubkey: owner,
      viewerPubkey: owner.toUpperCase(),
    })?.tag,
    hostDevice,
  );
  assert.equal(
    agentDeviceRobotVariant({
      hostDevice,
      ownerPubkey: owner,
      viewerPubkey: "b".repeat(64),
    }),
    null,
  );
  assert.equal(
    agentDeviceRobotVariant({
      hostDevice,
      ownerPubkey: null,
      viewerPubkey: owner,
    }),
    null,
  );
  assert.equal(
    agentDeviceRobotVariant({
      hostDevice: null,
      ownerPubkey: owner,
      viewerPubkey: owner,
    }),
    null,
  );
});
