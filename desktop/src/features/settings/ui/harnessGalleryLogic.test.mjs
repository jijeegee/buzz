import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  isEditableEntry,
  countAgentsReferencingHarness,
  deleteHarnessConfirmMessage,
  deleteConfirmState,
} from "./harnessGalleryLogic.ts";

// ── Minimal catalog entry factory ────────────────────────────────────────────

function entry(overrides = {}) {
  return {
    id: overrides.id ?? "test-id",
    label: overrides.label ?? "Test",
    source: overrides.source ?? "custom",
    availability: overrides.availability ?? "not_installed",
    avatarUrl: "",
    command: overrides.command ?? null,
    binaryPath: null,
    defaultArgs: [],
    mcpCommand: null,
    modelEnvVar: null,
    providerEnvVar: null,
    thinkingEnvVar: null,
    installHint: "",
    installInstructionsUrl: "",
    canAutoInstall: false,
    underlyingCliPath: null,
    nodeRequired: false,
    authStatus: { status: "not_applicable" },
    loginHint: null,
  };
}

// ── isEditableEntry ───────────────────────────────────────────────────────────

describe("isEditableEntry", () => {
  it("returns true for custom entries", () => {
    assert.ok(isEditableEntry(entry({ source: "custom" })));
  });

  it("returns false for preset entries", () => {
    assert.ok(!isEditableEntry(entry({ source: "preset" })));
  });

  it("returns false for builtin entries", () => {
    assert.ok(!isEditableEntry(entry({ source: "builtin" })));
  });
});

// ── countAgentsReferencingHarness ─────────────────────────────────────────────

describe("countAgentsReferencingHarness", () => {
  it("counts agents whose own runtime is the harness", () => {
    const agents = [
      { runtime: "my-harness", sourcePresetId: null },
      { runtime: "other", sourcePresetId: null },
      { runtime: "my-harness", sourcePresetId: "p1" },
    ];
    assert.equal(countAgentsReferencingHarness("my-harness", agents), 2);
  });

  it("agents with no runtime do not count, whatever preset they came from", () => {
    // The source preset is a label only; agents never inherit its runtime.
    const agents = [
      { runtime: null, sourcePresetId: null },
      { runtime: null, sourcePresetId: "p1" },
    ];
    assert.equal(countAgentsReferencingHarness("my-harness", agents), 0);
  });
});

// ── deleteHarnessConfirmMessage ───────────────────────────────────────────────

describe("deleteHarnessConfirmMessage", () => {
  it("plain confirmation when nothing references the harness", () => {
    assert.equal(
      deleteHarnessConfirmMessage("My Harness", 0),
      "Delete My Harness?",
    );
  });

  it("singular copy for one referencing agent", () => {
    assert.equal(
      deleteHarnessConfirmMessage("My Harness", 1),
      "1 agent uses this harness and will stop launching. Delete My Harness?",
    );
  });

  it("plural copy for multiple referencing agents", () => {
    assert.equal(
      deleteHarnessConfirmMessage("My Harness", 3),
      "3 agents use this harness and will stop launching. Delete My Harness?",
    );
  });
});

// ── deleteConfirmState ────────────────────────────────────────────────────────

describe("deleteConfirmState", () => {
  const settled = (data) => ({ isPending: false, isError: false, data });
  const pending = { isPending: true, isError: false, data: undefined };
  const failed = { isPending: false, isError: true, data: undefined };

  it("disables confirm while agents query is still loading", () => {
    const state = deleteConfirmState("h1", "My Harness", pending);
    assert.equal(state.canConfirm, false);
    assert.match(state.message, /Checking which agents/);
  });

  it("query failure does not claim zero dependents", () => {
    const state = deleteConfirmState("h1", "My Harness", failed);
    assert.equal(state.canConfirm, true);
    assert.match(state.message, /Couldn't check/);
    assert.doesNotMatch(state.message, /^Delete My Harness\?$/);
  });

  it("settled query produces the counted warning and enables confirm", () => {
    const agents = settled([
      { runtime: "h1", sourcePresetId: null },
      { runtime: "h1", sourcePresetId: "p1" },
      { runtime: null, sourcePresetId: "p1" },
    ]);
    const state = deleteConfirmState("h1", "My Harness", agents);
    assert.equal(state.canConfirm, true);
    assert.equal(
      state.message,
      "2 agents use this harness and will stop launching. Delete My Harness?",
    );
  });

  it("settled query with zero dependents uses the plain confirmation", () => {
    const state = deleteConfirmState("h1", "My Harness", settled([]));
    assert.equal(state.canConfirm, true);
    assert.equal(state.message, "Delete My Harness?");
  });
});
