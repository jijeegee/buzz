import assert from "node:assert/strict";
import test from "node:test";

import {
  submitProfilePersonaDialog,
  validateLinkedAgentRuntimeEdit,
} from "./UserProfilePanelPersonaSubmit.ts";

function agent(overrides = {}) {
  return {
    pubkey: "deadbeef".repeat(8),
    name: "Fizz",
    personaId: "persona-1",
    relayUrl: "ws://localhost:3000",
    acpCommand: "kuzz-acp",
    agentCommand: "goose",
    agentArgs: [],
    mcpCommand: "",
    turnTimeoutSeconds: 320,
    idleTimeoutSeconds: null,
    maxTurnDurationSeconds: null,
    parallelism: 1,
    systemPrompt: "Prompt",
    avatarUrl: null,
    model: null,
    envVars: {},
    status: "stopped",
    pid: null,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    lastStartedAt: null,
    lastStoppedAt: null,
    lastExitCode: null,
    lastError: null,
    logPath: null,
    startOnAppLaunch: true,
    backend: { type: "local" },
    backendAgentId: null,
    respondTo: "owner-only",
    respondToAllowlist: [],
    ...overrides,
  };
}

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

function runtime(overrides = {}) {
  return {
    id: "claude",
    label: "Claude Code",
    avatarUrl: "",
    availability: "available",
    command: "claude",
    binaryPath: "/usr/local/bin/claude",
    defaultArgs: [],
    mcpCommand: null,
    installHint: "",
    installInstructionsUrl: "",
    canAutoInstall: false,
    underlyingCliPath: null,
    ...overrides,
  };
}

test("validateLinkedAgentRuntimeEdit allows available runtime changes", () => {
  assert.equal(
    validateLinkedAgentRuntimeEdit({
      input: updateInput({ runtime: "claude" }),
      managedAgent: agent(),
      previousPersona: persona({ runtime: "goose" }),
      runtimes: [runtime()],
    }),
    null,
  );
});

test("validateLinkedAgentRuntimeEdit rejects unavailable linked-agent runtime changes", () => {
  assert.equal(
    validateLinkedAgentRuntimeEdit({
      input: updateInput({ runtime: "claude" }),
      managedAgent: agent(),
      previousPersona: persona({ runtime: "goose" }),
      runtimes: [runtime({ availability: "cli_missing", command: null })],
    }),
    "Claude Code is not available. Install it before saving this linked agent.",
  );
});

test("validateLinkedAgentRuntimeEdit allows unchanged or unlinked runtime preferences", () => {
  assert.equal(
    validateLinkedAgentRuntimeEdit({
      input: updateInput({ runtime: "goose" }),
      managedAgent: agent(),
      previousPersona: persona({ runtime: "goose" }),
      runtimes: [],
    }),
    null,
  );

  assert.equal(
    validateLinkedAgentRuntimeEdit({
      input: updateInput({ runtime: "claude" }),
      managedAgent: undefined,
      previousPersona: persona({ runtime: "goose" }),
      runtimes: [],
    }),
    null,
  );
});

test("submitProfilePersonaDialog follows the copy an edited built-in saves to", async () => {
  const copy = persona({ id: "copy-1", displayName: "My Fizz" });
  const agentUpdates = [];
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
    managedAgent: agent({ personaId: "builtin:fizz" }),
    onDone: () => {
      done += 1;
    },
    onPersonaReplaced: (next) => replaced.push(next.id),
    previousPersona: persona({ id: "builtin:fizz", isBuiltIn: true }),
    runtimes: [runtime({ id: "goose" })],
    updateManagedAgent: async (input) => {
      agentUpdates.push(input);
      return { agent: agent(), profileSyncError: null };
    },
    updatePersona: async () => copy,
  });

  // The agent now made from the copy still gets the edit, and the panel is
  // told to follow the copy.
  assert.equal(agentUpdates.length, 1);
  assert.equal(agentUpdates[0].name, "My Fizz");
  assert.deepEqual(replaced, ["copy-1"]);
  assert.equal(done, 1);
});

test("submitProfilePersonaDialog reports no replacement for an ordinary edit", async () => {
  const replaced = [];
  await submitProfilePersonaDialog({
    createManagedAgentForPersona: async () => assert.fail("no create"),
    createPersona: async () => assert.fail("no create"),
    input: updateInput({ runtime: "goose" }),
    managedAgent: undefined,
    onDone: () => {},
    onPersonaReplaced: (next) => replaced.push(next.id),
    updateManagedAgent: async () => assert.fail("no linked agent"),
    updatePersona: async () => persona(),
  });
  assert.deepEqual(replaced, []);
});
