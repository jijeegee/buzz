import assert from "node:assert/strict";
import test from "node:test";

import { submitProfilePersonaDialog } from "./UserProfilePanelPersonaSubmit.ts";

function persona(overrides = {}) {
  return {
    id: "persona-1",
    displayName: "Fizz",
    avatarUrl: null,
    systemPrompt: "Prompt",
    runtime: "goose",
    model: null,
    provider: null,
    namePool: [],
    isBuiltIn: false,
    isActive: true,
    envVars: {},
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

function updateInput(overrides = {}) {
  return {
    id: "persona-1",
    displayName: "Fizz",
    avatarUrl: undefined,
    systemPrompt: "Prompt",
    runtime: "claude",
    model: undefined,
    provider: undefined,
    namePool: [],
    ...overrides,
  };
}

test("submitProfilePersonaDialog follows the copy an edited built-in saves to", async () => {
  const copy = persona({ id: "copy-1", displayName: "My Fizz" });
  const replaced = [];
  let done = 0;
  await submitProfilePersonaDialog({
    createManagedAgentForPersona: async () => assert.fail("no create"),
    createPersona: async () => assert.fail("no create"),
    input: updateInput({
      id: "builtin:fizz",
      displayName: "My Fizz",
      runtime: "goose",
    }),
    onDone: () => {
      done += 1;
    },
    onPersonaReplaced: (next) => replaced.push(next.id),
    updatePersona: async () => copy,
  });

  // The panel is told to follow the copy.
  assert.deepEqual(replaced, ["copy-1"]);
  assert.equal(done, 1);
});

test("submitProfilePersonaDialog reports no replacement for an ordinary edit", async () => {
  const replaced = [];
  await submitProfilePersonaDialog({
    createManagedAgentForPersona: async () => assert.fail("no create"),
    createPersona: async () => assert.fail("no create"),
    input: updateInput({ runtime: "goose" }),
    onDone: () => {},
    onPersonaReplaced: (next) => replaced.push(next.id),
    updatePersona: async () => persona(),
  });
  assert.deepEqual(replaced, []);
});

test("submitProfilePersonaDialog edits only the preset, never agents made from it", async () => {
  const updated = [];
  await submitProfilePersonaDialog({
    createManagedAgentForPersona: async () => assert.fail("no create"),
    createPersona: async () => assert.fail("no create"),
    input: updateInput({ systemPrompt: "New prompt" }),
    onDone: () => {},
    // A stray agent writer must never be called by the preset save.
    updateManagedAgent: async () =>
      assert.fail("a preset edit never updates agents"),
    updatePersona: async (input) => {
      updated.push(input.id);
      return persona({ systemPrompt: "New prompt" });
    },
  });
  assert.deepEqual(updated, ["persona-1"]);
});
