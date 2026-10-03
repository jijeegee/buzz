import assert from "node:assert/strict";
import test from "node:test";

// The create forms used to drop the "Add your default AI" row entirely when
// nothing was starred, so turning on the Agents setting looked like a no-op.
// These tests pin the shared row: with a starred agent it is a live switch
// naming the agent; without one it stays visible, disabled, off, and points
// at the Runtime tab where the star lives.

const { fireEvent } = await import("@testing-library/react");
const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { AddDefaultAiRow } = await import("./AddDefaultAiRow.tsx");
const { NO_DEFAULT_AI_HINT } = await import("../lib/defaultAi.ts");

async function mount(props) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(
      React.createElement(AddDefaultAiRow, {
        checked: true,
        disabled: false,
        idPrefix: "create-channel",
        joinTarget: "this channel",
        onCheckedChange: () => {},
        ...props,
      }),
    );
  });
  return {
    container,
    row: () =>
      container.querySelector(
        "[data-testid='create-channel-default-ai-container']",
      ),
    switchEl: () =>
      container.querySelector("[data-testid='create-channel-add-default-ai']"),
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

test("with a starred agent the row is a live switch that names the agent", async () => {
  const changes = [];
  const { row, switchEl, unmount } = await mount({
    defaultAi: { name: "Scout" },
    onCheckedChange: (value) => changes.push(value),
  });

  assert.equal(row().getAttribute("data-default-ai"), "set");
  assert.ok(row().textContent.includes("Scout joins this channel as a bot"));
  assert.equal(switchEl().getAttribute("aria-checked"), "true");
  assert.equal(switchEl().hasAttribute("disabled"), false);
  // The sub-copy is the switch's accessible description.
  const hintId = switchEl().getAttribute("aria-describedby");
  assert.equal(hintId, "create-channel-add-default-ai-hint");
  assert.ok(
    document.getElementById(hintId).textContent.includes("Scout joins"),
  );

  await act(async () => {
    fireEvent.click(switchEl());
  });
  assert.deepEqual(changes, [false]);
  await unmount();
});

test("without a starred agent the row stays visible but disabled, off, and explains where to star one", async () => {
  const { row, switchEl, unmount } = await mount({
    checked: true,
    defaultAi: null,
    onCheckedChange: () => assert.fail("a disabled switch must not fire"),
  });

  assert.ok(row(), "the row renders even with nothing to add");
  assert.equal(row().getAttribute("data-default-ai"), "missing");
  assert.ok(row().textContent.includes("Add your default AI"));
  assert.ok(row().textContent.includes(NO_DEFAULT_AI_HINT));
  assert.ok(
    document
      .getElementById(switchEl().getAttribute("aria-describedby"))
      .textContent.includes(NO_DEFAULT_AI_HINT),
    "the hint is what the disabled switch is described by",
  );
  // The stored preference may be on, but the switch mirrors the payload
  // `resolveAddDefaultAi` will send: false.
  assert.equal(switchEl().getAttribute("aria-checked"), "false");
  assert.equal(switchEl().hasAttribute("disabled"), true);

  await act(async () => {
    fireEvent.click(switchEl());
  });
  await unmount();
});

test("the project form variant keeps its own ids and join copy", async () => {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(
      React.createElement(AddDefaultAiRow, {
        checked: false,
        defaultAi: { name: "Scout" },
        disabled: true,
        idPrefix: "create-project",
        joinTarget: "the project home",
        onCheckedChange: () => {},
      }),
    );
  });

  const row = container.querySelector(
    "[data-testid='create-project-default-ai-container']",
  );
  assert.ok(row);
  assert.ok(row.textContent.includes("Scout joins the project home as a bot"));
  const switchEl = container.querySelector(
    "[data-testid='create-project-add-default-ai']",
  );
  assert.equal(switchEl.id, "create-project-add-default-ai");
  assert.equal(
    container.querySelector("label").getAttribute("for"),
    "create-project-add-default-ai",
  );
  // `disabled` (form submitting) also disables the switch.
  assert.equal(switchEl.hasAttribute("disabled"), true);
  await act(async () => root.unmount());
  container.remove();
});
