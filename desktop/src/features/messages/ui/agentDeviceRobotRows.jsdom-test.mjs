import assert from "node:assert/strict";
import { afterEach, before, test } from "node:test";

import { cleanup, render } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement } from "react";

import { usersBatchEntryKey } from "@/features/profile/hooks";
import { ownerDevicesQueryKey } from "@/shared/api/useOwnerDevices";
import { deviceRobotVariantForDevice } from "@/shared/lib/deviceRobot";
import { TooltipProvider } from "@/shared/ui/tooltip";
import { MentionAutocomplete } from "./MentionAutocomplete.tsx";
import { MessageAgentOwner } from "./MessageAgentOwner.tsx";
import { NewMessageResultRow } from "./NewMessageResultRow.tsx";

// Rows that only know the agent's owner from the row itself (mention
// candidates, directory search results) draw the same shared agent badge a
// chat message does: the device robot with its name on hover for the owner,
// the owner mark for everyone else.

const OWNER = "a".repeat(64);
const OTHER = "b".repeat(64);
const AGENT = "c".repeat(64);
const DEVICE_ID = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";

before(() => {
  window.HTMLElement.prototype.scrollIntoView ??= () => {};
  globalThis.ResizeObserver ??= class {
    disconnect() {}
    observe() {}
    unobserve() {}
  };
});

afterEach(() => cleanup());

function withProviders(viewer, child, { cacheAgentProfile = false } = {}) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  queryClient.setQueryData(["identity"], { pubkey: viewer });
  for (const pubkey of [OWNER, OTHER]) {
    queryClient.setQueryData(ownerDevicesQueryKey(pubkey), {
      hostDevices: new Map([[AGENT, DEVICE_ID]]),
      robotOverrides: new Map(),
    });
  }
  queryClient.setQueryData(
    ["auth-devices"],
    [{ id: DEVICE_ID, name: "Laptop" }],
  );
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
  if (cacheAgentProfile) {
    queryClient.setQueryData(usersBatchEntryKey(AGENT), {
      fetchedAt: Date.now(),
      summary: {
        avatarUrl: null,
        displayName: "Agent Ada",
        isAgent: true,
        nip05Handle: null,
        ownerPubkey: OWNER,
      },
    });
  }
  return createElement(
    QueryClientProvider,
    { client: queryClient },
    createElement(TooltipProvider, null, child),
  );
}

function renderMention(viewer, options) {
  return render(
    withProviders(
      viewer,
      createElement(MentionAutocomplete, {
        suggestions: [
          {
            pubkey: AGENT,
            displayName: "Agent Ada",
            isAgent: true,
            ownerLabel: viewer === OWNER ? "you" : "Owner",
            ownerPubkey: OWNER,
          },
        ],
        selectedIndex: 0,
        onSelect: () => {},
      }),
      options,
    ),
  );
}

function renderMessageBadge(viewer) {
  const view = render(
    withProviders(
      viewer,
      createElement(MessageAgentOwner, {
        agentPubkey: AGENT,
        ownerLabel: "Owner",
        ownerPubkey: OWNER,
      }),
      { cacheAgentProfile: true },
    ),
  );
  return view.container.querySelector("[data-agent-badge]");
}

/** The badge's own structure, minus the row-specific size and test id. */
function badgeShape(element) {
  const clone = element.cloneNode(true);
  clone.removeAttribute("class");
  clone.removeAttribute("data-testid");
  return clone.outerHTML;
}

test("mention autocomplete: the owner sees the host device robot", () => {
  const view = renderMention(OWNER);
  const icon = view.getByTestId("mention-agent-icon");
  assert.equal(icon.getAttribute("data-agent-badge"), "device");
  assert.equal(icon.getAttribute("data-device-id"), DEVICE_ID);
  assert.equal(icon.getAttribute("aria-label"), "Running on Laptop");
  const robot = icon.querySelector("[data-testid=device-robot-icon]");
  assert.equal(
    robot?.getAttribute("data-robot-tag"),
    deviceRobotVariantForDevice(DEVICE_ID).tag,
  );
  assert.match(
    // icon → "agent" chip → the row's metadata line.
    icon.parentElement?.parentElement?.textContent ?? "",
    /agentmanaged by you/,
  );
});

test("mention autocomplete: other viewers see the owner mark", () => {
  const view = renderMention(OTHER);
  const icon = view.getByTestId("mention-agent-icon");
  assert.equal(icon.getAttribute("data-agent-badge"), "owner");
  assert.equal(icon.getAttribute("data-owner-pubkey"), OWNER);
  assert.equal(icon.getAttribute("title"), "Agent owned by Owner");
  assert.equal(icon.getAttribute("data-device-id"), null);
});

test("mention autocomplete draws exactly the chat message's badge", () => {
  for (const viewer of [OWNER, OTHER]) {
    const fromMessage = badgeShape(renderMessageBadge(viewer));
    cleanup();
    for (const cacheAgentProfile of [true, false]) {
      const view = renderMention(viewer, { cacheAgentProfile });
      assert.equal(
        badgeShape(view.getByTestId("mention-agent-icon")),
        fromMessage,
        `viewer ${viewer.slice(0, 1)}, profile cached: ${cacheAgentProfile}`,
      );
      cleanup();
    }
  }
});

function renderResultRow(viewer) {
  return render(
    withProviders(
      viewer,
      createElement(NewMessageResultRow, {
        currentPubkey: viewer,
        disabled: false,
        onSelect: () => {},
        user: {
          pubkey: AGENT,
          displayName: "Agent Ada",
          avatarUrl: null,
          nip05Handle: null,
          ownerPubkey: OWNER,
          isAgent: true,
        },
      }),
    ),
  );
}

test("new message row: the same badge as a chat message, per viewer", () => {
  for (const viewer of [OWNER, OTHER]) {
    const fromMessage = badgeShape(renderMessageBadge(viewer));
    cleanup();
    const view = renderResultRow(viewer);
    assert.equal(
      badgeShape(view.getByTestId("new-dm-agent-icon")),
      fromMessage,
    );
    cleanup();
  }
});
