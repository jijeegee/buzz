import assert from "node:assert/strict";
import test from "node:test";

// The Runtime tab's "Default AI" row is the only user-reachable default-AI
// control (see UserProfileDefaultAiWiring.test.mjs for where it is mounted).
// These tests pin the row itself: it mirrors `isDefaultAi`, click and
// Enter/Space each toggle exactly once, and while an agent action is pending
// it ignores toggles and shows a disabled switch.

const { fireEvent } = await import("@testing-library/react");
const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { ProfileDefaultAiRow } = await import("./ProfileDefaultAiRow.tsx");

async function mount(props) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(
      React.createElement(ProfileDefaultAiRow, {
        checked: false,
        onToggle: () => {},
        ...props,
      }),
    );
  });
  return {
    container,
    row: () =>
      container.querySelector("[data-testid='user-profile-default-ai']"),
    toggle: () =>
      container.querySelector("[data-testid='user-profile-default-ai-toggle']"),
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

test("the row is one labelled switch that mirrors the star", async () => {
  const { row, toggle, unmount } = await mount({ checked: true });

  assert.equal(row().getAttribute("role"), "switch");
  assert.equal(row().getAttribute("aria-label"), "Default AI");
  assert.equal(row().getAttribute("aria-checked"), "true");
  assert.equal(row().getAttribute("aria-disabled"), "false");
  assert.equal(row().getAttribute("tabindex"), "0");
  assert.ok(row().textContent.includes("Default AI"));
  // The visual switch is decorative; the row carries the accessible name.
  assert.equal(toggle().getAttribute("aria-hidden"), "true");
  assert.equal(toggle().getAttribute("aria-checked"), "true");
  assert.equal(toggle().hasAttribute("disabled"), false);
  await unmount();
});

test("clicking the row or pressing Enter/Space toggles exactly once each", async () => {
  const toggles = [];
  const { row, unmount } = await mount({
    checked: false,
    onToggle: () => toggles.push("toggle"),
  });

  assert.equal(row().getAttribute("aria-checked"), "false");
  await act(async () => {
    fireEvent.click(row());
  });
  assert.deepEqual(toggles, ["toggle"]);

  await act(async () => {
    fireEvent.keyDown(row(), { key: "Enter" });
  });
  await act(async () => {
    fireEvent.keyDown(row(), { key: " " });
  });
  await act(async () => {
    fireEvent.keyDown(row(), { key: "a" });
  });
  assert.deepEqual(toggles, ["toggle", "toggle", "toggle"]);
  await unmount();
});

test("while an agent action is pending the row ignores toggles and its switch is disabled", async () => {
  const { row, toggle, unmount } = await mount({
    checked: true,
    onToggle: () => assert.fail("must not toggle while pending"),
    pending: true,
  });

  assert.equal(row().getAttribute("aria-disabled"), "true");
  assert.equal(toggle().hasAttribute("disabled"), true);
  await act(async () => {
    fireEvent.click(row());
  });
  await act(async () => {
    fireEvent.keyDown(row(), { key: "Enter" });
  });
  await unmount();
});
