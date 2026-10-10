import assert from "node:assert/strict";
import test from "node:test";

import { DEVICE_ROBOT_SHAPES } from "./deviceRobot.ts";
import {
  deviceRobotChipStyleText,
  deviceRobotMaskUrl,
  ownAgentDeviceRobot,
} from "./deviceRobotMask.ts";

test("every robot shape builds a CSS-safe mask url", () => {
  for (const shape of DEVICE_ROBOT_SHAPES) {
    const url = deviceRobotMaskUrl(shape);
    assert.match(url, /^url\("data:image\/svg\+xml,%3Csvg /);
    // Raw `<`, `>` or `#` would break the data URL inside a style attribute.
    assert.doesNotMatch(url, /[<>#]/);
  }
});

test("only the viewer's own agents resolve a device robot", () => {
  const agent = "a".repeat(64);
  const ownerDevices = {
    hostDevices: new Map([[agent, "device-1"]]),
    robotOverrides: new Map([["device-1", { shape: "dome", colorIndex: 2 }]]),
  };
  const robot = ownAgentDeviceRobot(agent.toUpperCase(), ownerDevices);
  assert.equal(robot?.shape, "dome");
  assert.equal(robot?.colorIndex, 2);
  assert.match(deviceRobotChipStyleText(robot), /--agent-robot-color: #/);
  assert.equal(ownAgentDeviceRobot("b".repeat(64), ownerDevices), null);
  assert.equal(ownAgentDeviceRobot(null, ownerDevices), null);
});
