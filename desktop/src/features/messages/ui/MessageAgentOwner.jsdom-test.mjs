import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

import { cleanup, render } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement } from "react";

import { usersBatchEntryKey } from "@/features/profile/hooks";
import { deviceRobotVariantFromTag } from "@/shared/lib/deviceRobot";
import { TooltipProvider } from "@/shared/ui/tooltip";
import { MessageAgentOwner } from "./MessageAgentOwner.tsx";

const OWNER = "a".repeat(64);
const AGENT = "c".repeat(64);
const TAG = "e8c41c31";

afterEach(() => cleanup());

function renderOwner({ viewer, hostDevice = TAG, ownerPubkey = OWNER }) {
  const queryClient = new QueryClient();
  // What `useUsersBatchQuery` leaves behind for a resolved author, and the
  // signed-in identity: the robot needs no fetch of its own.
  queryClient.setQueryData(["identity"], { pubkey: viewer });
  queryClient.setQueryData(usersBatchEntryKey(AGENT), {
    fetchedAt: Date.now(),
    summary: {
      avatarUrl: null,
      displayName: "Agent",
      hostDevice,
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
  const expected = deviceRobotVariantFromTag(TAG);
  assert.equal(robot.getAttribute("data-robot-tag"), TAG);
  assert.equal(robot.getAttribute("data-robot-shape"), expected.shape);
  assert.equal(robot.getAttribute("aria-hidden"), "true");
});

test("anyone else sees the default robot", () => {
  const { container } = renderOwner({ viewer: "b".repeat(64) });
  assert.equal(
    container.querySelector("[data-testid=device-robot-icon]"),
    null,
  );
  assert.ok(container.querySelector("svg.lucide-bot"), "default bot shown");
});

test("an agent without a host device shows the default robot", () => {
  const { container } = renderOwner({ viewer: OWNER, hostDevice: null });
  assert.equal(
    container.querySelector("[data-testid=device-robot-icon]"),
    null,
  );
  assert.ok(container.querySelector("svg.lucide-bot"), "default bot shown");
});
