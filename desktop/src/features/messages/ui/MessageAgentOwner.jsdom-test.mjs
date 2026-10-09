import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement } from "react";

import { usersBatchEntryKey } from "@/features/profile/hooks";
import { ownerDevicesQueryKey } from "@/shared/api/useOwnerDevices";
import {
  DEVICE_ROBOT_COLORS,
  deviceRobotVariantForDevice,
} from "@/shared/lib/deviceRobot";
import { TooltipProvider } from "@/shared/ui/tooltip";
import { MessageAgentOwner } from "./MessageAgentOwner.tsx";

const OWNER = "a".repeat(64);
const AGENT = "c".repeat(64);
const DEVICE_ID = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";

afterEach(() => cleanup());

function renderOwner({
  viewer,
  hostDevices = new Map([[AGENT, DEVICE_ID]]),
  robotOverrides = new Map(),
  deviceNames = [{ id: DEVICE_ID, name: "Studio PC" }],
  ownerPubkey = OWNER,
}) {
  const queryClient = new QueryClient();
  // What `useUsersBatchQuery` leaves behind for a resolved author, the
  // signed-in identity, the owner's host-device map and device list: the
  // robot needs no fetch of its own.
  queryClient.setQueryData(["identity"], { pubkey: viewer });
  queryClient.setQueryData(ownerDevicesQueryKey(viewer), {
    hostDevices,
    robotOverrides,
  });
  queryClient.setQueryData(["auth-devices"], deviceNames);
  queryClient.setQueryData(usersBatchEntryKey(OWNER), {
    fetchedAt: Date.now(),
    summary: {
      avatarUrl: null,
      displayName: "Owner",
      isAgent: false,
      nip05Handle: null,
      ownerPubkey: null,
    },
  });
  queryClient.setQueryData(usersBatchEntryKey(AGENT), {
    fetchedAt: Date.now(),
    summary: {
      avatarUrl: null,
      displayName: "Agent",
      isAgent: true,
      nip05Handle: null,
      ownerPubkey,
    },
  });
  return render(
    createElement(
      QueryClientProvider,
      { client: queryClient },
      createElement(
        TooltipProvider,
        null,
        createElement(MessageAgentOwner, {
          agentPubkey: AGENT,
          ownerLabel: "You",
          ownerPubkey: OWNER,
        }),
      ),
    ),
  );
}

test("the owner sees the robot of the device the agent runs on", () => {
  const { container } = renderOwner({ viewer: OWNER });
  const robot = container.querySelector("[data-testid=device-robot-icon]");
  assert.ok(robot, "device robot rendered");
  const expected = deviceRobotVariantForDevice(DEVICE_ID);
  assert.equal(robot.getAttribute("data-robot-tag"), expected.tag);
  assert.equal(robot.getAttribute("data-robot-shape"), expected.shape);
  assert.equal(robot.getAttribute("aria-hidden"), "true");
  assert.equal(robot.parentElement.getAttribute("data-device-id"), DEVICE_ID);
});

test("hovering the owner's robot names the device it runs on", async () => {
  const { container, findByTestId } = renderOwner({ viewer: OWNER });
  const trigger = container.querySelector("[data-device-id]");
  assert.equal(trigger.getAttribute("aria-label"), "Running on Studio PC");
  await act(async () => {
    fireEvent.focus(trigger);
  });
  const tooltip = await findByTestId("agent-host-device-tooltip");
  assert.match(tooltip.textContent, /Running on Studio PC/);
});

test("an unnamed host device still gets a tooltip", () => {
  const { container } = renderOwner({ viewer: OWNER, deviceNames: [] });
  assert.equal(
    container.querySelector("[data-device-id]")?.getAttribute("aria-label"),
    "Running on another device",
  );
});

test("anyone else sees the owner mark instead of the device robot", () => {
  const { container } = renderOwner({ viewer: "b".repeat(64) });
  assert.equal(
    container.querySelector("[data-testid=device-robot-icon]"),
    null,
  );
  const mark = container.querySelector("[data-testid=agent-owner-mark]");
  assert.ok(mark, "owner mark rendered");
  assert.equal(mark.getAttribute("data-owner-pubkey"), OWNER);
  assert.equal(mark.getAttribute("title"), "Agent owned by Owner");
});

test("a non-owner never gets a device robot, even with a map entry", () => {
  // A stray map entry under another viewer's key must not leak a device.
  const { container } = renderOwner({
    viewer: "b".repeat(64),
    ownerPubkey: OWNER,
  });
  assert.equal(container.querySelector("[data-device-id]"), null);
});

test("an agent without a verified owner shows the default robot", () => {
  const { container } = renderOwner({
    viewer: "b".repeat(64),
    ownerPubkey: null,
  });
  assert.equal(container.querySelector("[data-testid=agent-owner-mark]"), null);
  assert.ok(container.querySelector("svg.lucide-bot"), "default bot shown");
});

test("an owned agent missing from the host-device map shows the default robot", () => {
  const { container } = renderOwner({ viewer: OWNER, hostDevices: new Map() });
  assert.equal(
    container.querySelector("[data-testid=device-robot-icon]"),
    null,
  );
  assert.ok(container.querySelector("svg.lucide-bot"), "default bot shown");
});

test("the owner's robot choice for the device replaces the hashed default", () => {
  const hashed = deviceRobotVariantForDevice(DEVICE_ID);
  const shape = hashed.shape === "hex" ? "dome" : "hex";
  const colorIndex = (hashed.colorIndex + 1) % 8;
  const { container } = renderOwner({
    viewer: OWNER,
    robotOverrides: new Map([[DEVICE_ID, { shape, colorIndex, createdAt: 1 }]]),
  });
  const robot = container.querySelector("[data-testid=device-robot-icon]");
  assert.equal(robot.getAttribute("data-robot-shape"), shape);
  assert.equal(robot.getAttribute("data-robot-tag"), hashed.tag);
  const probe = document.createElement("span");
  probe.style.color = DEVICE_ROBOT_COLORS[colorIndex];
  assert.equal(robot.style.color, probe.style.color);
});
