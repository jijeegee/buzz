import assert from "node:assert/strict";
import test from "node:test";

import { shouldEscapeCloseSettings } from "./settingsEscape.ts";

const noLayers = { querySelector: () => null };
const openLayer = { querySelector: () => ({}) };

function escapeEvent(target, overrides = {}) {
  return {
    key: "Escape",
    defaultPrevented: false,
    isComposing: false,
    target,
    ...overrides,
  };
}

const button = {
  tagName: "BUTTON",
  isContentEditable: false,
  closest: () => null,
};

test("Escape from plain focus closes Settings", () => {
  assert.equal(shouldEscapeCloseSettings(escapeEvent(button), noLayers), true);
  assert.equal(shouldEscapeCloseSettings(escapeEvent(null), noLayers), true);
});

test("Escape inside an editable field stays with the field", () => {
  for (const tagName of ["INPUT", "TEXTAREA", "SELECT", "input"]) {
    assert.equal(
      shouldEscapeCloseSettings(escapeEvent({ tagName }), noLayers),
      false,
      tagName,
    );
  }
  assert.equal(
    shouldEscapeCloseSettings(
      escapeEvent({ tagName: "DIV", isContentEditable: true }),
      noLayers,
    ),
    false,
  );
  assert.equal(
    shouldEscapeCloseSettings(
      escapeEvent({ tagName: "SPAN", closest: () => ({}) }),
      noLayers,
    ),
    false,
    "a node inside a contenteditable editor",
  );
});

test("Escape that an open dialog, popover or menu owns does not close Settings", () => {
  assert.equal(
    shouldEscapeCloseSettings(escapeEvent(button), openLayer),
    false,
  );
  assert.equal(
    shouldEscapeCloseSettings(
      escapeEvent(button, { defaultPrevented: true }),
      noLayers,
    ),
    false,
  );
});

test("other keys and IME composition never close Settings", () => {
  assert.equal(
    shouldEscapeCloseSettings(escapeEvent(button, { key: "Enter" }), noLayers),
    false,
  );
  assert.equal(
    shouldEscapeCloseSettings(
      escapeEvent(button, { isComposing: true }),
      noLayers,
    ),
    false,
  );
});
