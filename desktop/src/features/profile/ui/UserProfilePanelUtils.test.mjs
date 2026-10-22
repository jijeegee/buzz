import assert from "node:assert/strict";
import test from "node:test";

import {
  parseProfilePanelTab,
  parseProfilePanelView,
  profilePanelTabFromSearch,
  profilePanelTargetKey,
  profilePanelViewFromSearch,
  resolveAgentInstruction,
  resolvePanelProfile,
} from "./UserProfilePanelUtils.ts";

function agent(overrides = {}) {
  return {
    pubkey: "deadbeef".repeat(8),
    name: "Fizz",
    sourcePresetId: "persona-1",
    relayUrl: "ws://localhost:3000",
    acpCommand: "kuzz-acp",
    agentCommand: "goose",
    agentArgs: [],
    mcpCommand: "",
    turnTimeoutSeconds: 320,
    idleTimeoutSeconds: null,
    maxTurnDurationSeconds: null,
    parallelism: 1,
    systemPrompt: "Old prompt",
    avatarUrl: "app-avatar://old",
    model: "old-model",
    envVars: { OLD_KEY: "1" },
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
    displayName: "Fizz Prime",
    avatarUrl: null,
    systemPrompt: "New prompt",
    runtime: "goose",
    model: "new-model",
    provider: null,
    namePool: [],
    isBuiltIn: false,
    isActive: true,
    respondTo: "owner-only",
    respondToAllowlist: [],
    envVars: { NEW_KEY: "2" },
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

test("an agent made from a preset shows its own instructions, not the preset's", () => {
  assert.equal(
    resolveAgentInstruction(agent({ systemPrompt: " Old prompt " }), persona()),
    "Old prompt",
  );
  assert.equal(
    resolveAgentInstruction(agent({ systemPrompt: null }), persona()),
    null,
    "a blank agent prompt never falls back to the preset's",
  );
  assert.equal(resolveAgentInstruction(undefined, persona()), "New prompt");
});

test("an agent's profile falls back to its own avatar, never its preset's", () => {
  const presetAvatar = persona({ avatarUrl: "app-avatar://preset" });
  assert.equal(
    resolvePanelProfile({
      managedAgent: agent({ avatarUrl: null }),
      persona: presetAvatar,
      profile: undefined,
    })?.avatarUrl ?? null,
    null,
  );
  assert.equal(
    resolvePanelProfile({
      managedAgent: agent(),
      persona: presetAvatar,
      profile: undefined,
    })?.avatarUrl,
    "app-avatar://old",
  );
  assert.equal(
    resolvePanelProfile({
      managedAgent: undefined,
      persona: presetAvatar,
      profile: undefined,
    })?.avatarUrl,
    "app-avatar://preset",
    "a preset profile still shows the preset's avatar",
  );
});

test("parseProfilePanelView accepts all profile panel subviews", () => {
  for (const view of [
    "summary",
    "info",
    "configuration",
    "diagnostics",
    "memories",
    "channels",
    "logs",
  ]) {
    assert.equal(parseProfilePanelView(view), view);
  }
});

test("parseProfilePanelView maps legacy agent config subviews to configuration", () => {
  for (const view of ["model", "settings"]) {
    assert.equal(parseProfilePanelView(view), "configuration");
  }
});

test("profilePanelViewFromSearch falls back to summary for invalid values", () => {
  assert.equal(parseProfilePanelView("missing"), null);
  assert.equal(profilePanelViewFromSearch("missing"), "summary");
  assert.equal(profilePanelViewFromSearch(null), "summary");
});

test("parseProfilePanelTab accepts profile summary tabs", () => {
  for (const tab of ["info", "runtime", "channels", "memories"]) {
    assert.equal(parseProfilePanelTab(tab), tab);
  }
});

test("profilePanelTabFromSearch falls back to info for invalid values", () => {
  assert.equal(parseProfilePanelTab("missing"), null);
  assert.equal(profilePanelTabFromSearch("missing"), "info");
  assert.equal(profilePanelTabFromSearch(null), "info");
});

test("profile target identity stays stable while a requested pubkey is canonicalized", () => {
  const historicalPubkey = "a".repeat(64);
  assert.equal(
    profilePanelTargetKey(historicalPubkey, undefined),
    historicalPubkey,
  );
  assert.equal(
    profilePanelTargetKey(historicalPubkey, "resolved-persona"),
    historicalPubkey,
  );
  assert.equal(
    profilePanelTargetKey(undefined, "requested-persona"),
    "persona:requested-persona",
  );
});
