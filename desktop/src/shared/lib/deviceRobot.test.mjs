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
  deviceRobotContent,
  parseDeviceRobotContent,
  resolveDeviceRobotVariant,
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
  const device = GOLDEN.devices[0];
  const mine = agentBadge({
    hostDeviceId: device.deviceId.toUpperCase(),
    ownerPubkey: owner,
    viewerPubkey: owner.toUpperCase(),
  });
  assert.equal(mine.kind, "device");
  assert.equal(mine.deviceId, device.deviceId.trim().toLowerCase());
  assert.equal(mine.variant.tag, device.tag);
  assert.deepEqual(
    mine.variant,
    deviceRobotVariantForDevice(device.deviceId),
    "same robot as the device list row",
  );
  assert.deepEqual(
    agentBadge({
      hostDeviceId: device.deviceId,
      ownerPubkey: owner.toUpperCase(),
      viewerPubkey: "b".repeat(64),
    }),
    { kind: "owner", ownerPubkey: owner },
  );
  assert.deepEqual(
    agentBadge({
      hostDeviceId: null,
      ownerPubkey: owner,
      viewerPubkey: "b".repeat(64),
    }),
    { kind: "owner", ownerPubkey: owner },
    "others see the owner even without a host device",
  );
  for (const input of [
    { hostDeviceId: device.deviceId, ownerPubkey: null, viewerPubkey: owner },
    { hostDeviceId: null, ownerPubkey: owner, viewerPubkey: owner },
    { hostDeviceId: "  ", ownerPubkey: owner, viewerPubkey: owner },
    { hostDeviceId: device.deviceId, ownerPubkey: owner, viewerPubkey: null },
  ]) {
    assert.deepEqual(agentBadge(input), { kind: "default" });
  }
});

test("a valid robot override wins over the hashed default", () => {
  const device = GOLDEN.devices[0];
  const hashed = deviceRobotVariantForDevice(device.deviceId);
  const shape = hashed.shape === "visor" ? "boxy" : "visor";
  const colorIndex = (hashed.colorIndex + 3) % DEVICE_ROBOT_COLORS.length;
  const resolved = resolveDeviceRobotVariant(device.deviceId, {
    shape,
    colorIndex,
  });
  assert.equal(resolved.shape, shape);
  assert.equal(resolved.shapeIndex, DEVICE_ROBOT_SHAPES.indexOf(shape));
  assert.equal(resolved.colorIndex, colorIndex);
  assert.equal(resolved.color, DEVICE_ROBOT_COLORS[colorIndex]);
  assert.equal(resolved.tag, hashed.tag, "the tag still names the device");
  assert.deepEqual(resolveDeviceRobotVariant(device.deviceId, null), hashed);
  assert.equal(resolveDeviceRobotVariant("", { shape, colorIndex }), null);

  const badge = agentBadge({
    hostDeviceId: device.deviceId,
    robotOverride: { shape, colorIndex },
    ownerPubkey: "a".repeat(64),
    viewerPubkey: "a".repeat(64),
  });
  assert.equal(badge.variant.shape, shape);
});

test("invalid robot overrides are ignored", () => {
  const device = GOLDEN.devices[0];
  const hashed = deviceRobotVariantForDevice(device.deviceId);
  for (const bad of [
    { shape: "blob", colorIndex: 1 },
    { shape: "dome", colorIndex: 8 },
    { shape: "dome", colorIndex: -1 },
  ]) {
    assert.deepEqual(
      resolveDeviceRobotVariant(device.deviceId, bad),
      hashed,
      JSON.stringify(bad),
    );
  }
  for (const content of [
    "",
    "{}",
    '{"v":1,"shape":"blob","color":1}',
    '{"v":1,"shape":"dome","color":8}',
    '{"v":1,"shape":"dome","color":2.5}',
    '{"v":1,"shape":"dome"}',
    '{"v":2,"shape":"dome","color":1}',
  ]) {
    assert.equal(parseDeviceRobotContent(content), null, content);
  }
  assert.deepEqual(
    parseDeviceRobotContent(
      deviceRobotContent({ shape: "hex", colorIndex: 7 }),
    ),
    { shape: "hex", colorIndex: 7 },
  );
  assert.equal(
    deviceRobotContent({ shape: "hex", colorIndex: 7 }),
    '{"v":1,"shape":"hex","color":7}',
  );
});
