import assert from "node:assert/strict";
import test from "node:test";

// The profile settings menu is the only user-reachable default-AI control
// (`ManagedAgentRow`/`AgentGroupRows` are not wired into any route). These
// tests pin that surface: the switch mirrors `isDefaultAi`, flipping it (or
// selecting its row) asks the owner for exactly one toggle, key-less records
// get no switch, and the switch follows the shared pending state.

const { fireEvent } = await import("@testing-library/react");
const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { UserProfileAgentSettingsMenu } = await import(
  "./UserProfileAgentActions.tsx"
);

const PUBKEY = "ab".repeat(32);
const SWITCH_ID = `user-profile-agent-default-ai-${PUBKEY}`;

function managedAgent(overrides = {}) {
  return {
    pubkey: PUBKEY,
    name: "Scout",
    backend: { type: "local" },
    startOnAppLaunch: false,
    isDefaultAi: false,
    ...overrides,
  };
}

async function mountMenu(props) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(React.createElement(UserProfileAgentSettingsMenu, props));
  });
  const trigger = container.querySelector(
    "[data-testid='user-profile-settings-menu-trigger']",
  );
  assert.ok(trigger, "settings menu trigger renders");
  // Radix opens the menu from the trigger's keydown; the content portals into
  // document.body, so queries below go through `document`.
  await act(async () => {
    fireEvent.keyDown(trigger, { key: "Enter" });
  });
  return {
    container,
    switchEl: () => document.querySelector(`[data-testid='${SWITCH_ID}']`),
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

test("the switch mirrors the record's star and flipping it toggles once", async () => {
  const toggles = [];
  const { switchEl, unmount } = await mountMenu({
    isPending: false,
    managedAgent: managedAgent({ isDefaultAi: true }),
    onToggleAutoStart: () => {},
    onToggleDefaultAi: () => toggles.push("toggle"),
  });

  const switchNode = switchEl();
  assert.ok(switchNode, "default-AI switch renders for a keyed agent");
  assert.equal(switchNode.getAttribute("role"), "switch");
  assert.equal(switchNode.getAttribute("aria-checked"), "true");
  assert.equal(switchNode.getAttribute("aria-label"), "Default AI");
  assert.equal(switchNode.hasAttribute("disabled"), false);

  await act(async () => {
    fireEvent.click(switchNode);
  });
  assert.deepEqual(
    toggles,
    ["toggle"],
    "the switch click must not also bubble into the row's onSelect",
  );
  await unmount();
});

test("an unstarred agent shows the switch off and the row itself toggles", async () => {
  const toggles = [];
  const { switchEl, unmount } = await mountMenu({
    isPending: false,
    managedAgent: managedAgent({ isDefaultAi: false }),
    onToggleDefaultAi: () => toggles.push("toggle"),
  });

  const switchNode = switchEl();
  assert.ok(switchNode);
  assert.equal(switchNode.getAttribute("aria-checked"), "false");

  const row = switchNode.closest("[role='menuitem']");
  assert.ok(row, "the switch sits inside its labelled menu row");
  await act(async () => {
    fireEvent.click(row);
  });
  assert.deepEqual(toggles, ["toggle"]);
  await unmount();
});

test("a key-less record gets no default-AI switch", async () => {
  const { container, switchEl, unmount } = await mountMenu({
    isPending: false,
    managedAgent: managedAgent({ pubkey: "" }),
    onToggleAutoStart: () => {},
    onToggleDefaultAi: () => assert.fail("must never be reachable"),
  });

  assert.ok(
    document.querySelector("[data-testid='user-profile-agent-auto-start-']"),
    "the menu still renders its other rows",
  );
  assert.equal(switchEl(), null);
  assert.equal(container.textContent.includes("Default AI"), false);
  await unmount();
});

test("without an owner handler the row is absent", async () => {
  const { switchEl, unmount } = await mountMenu({
    isPending: false,
    managedAgent: managedAgent({ isDefaultAi: true }),
    onToggleAutoStart: () => {},
  });

  assert.equal(switchEl(), null);
  assert.equal(document.body.textContent.includes("Default AI"), false);
  await unmount();
});

test("the switch is disabled while an agent action is pending", async () => {
  const { switchEl, unmount } = await mountMenu({
    isPending: true,
    managedAgent: managedAgent(),
    onToggleDefaultAi: () => {},
  });

  const switchNode = switchEl();
  assert.ok(switchNode);
  assert.equal(switchNode.hasAttribute("disabled"), true);
  await unmount();
});
