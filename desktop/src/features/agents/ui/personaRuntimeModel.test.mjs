import assert from "node:assert/strict";
import test from "node:test";

import {
  resolveRuntimeProviderCapability,
  shouldClearModelForRuntimeChange,
} from "./personaRuntimeModel.ts";

test("shouldClearModelForRuntimeChange preserves model for first runtime selection", () => {
  assert.equal(shouldClearModelForRuntimeChange("", "goose"), false);
});

test("shouldClearModelForRuntimeChange clears model when switching runtimes", () => {
  assert.equal(shouldClearModelForRuntimeChange("goose", "claude"), true);
});

test("shouldClearModelForRuntimeChange clears model when runtime is removed", () => {
  assert.equal(shouldClearModelForRuntimeChange("goose", ""), true);
});

test("shouldClearModelForRuntimeChange keeps model for unchanged runtime", () => {
  assert.equal(shouldClearModelForRuntimeChange("goose", "goose"), false);
});

test("resolveRuntimeProviderCapability classifies provider-capable runtimes as capable", () => {
  assert.equal(resolveRuntimeProviderCapability("kuzz-agent", true), "capable");
  assert.equal(resolveRuntimeProviderCapability("goose", true), "capable");
});

test("resolveRuntimeProviderCapability classifies known CLI-login runtimes as locked before the catalog loads", () => {
  // The core fix: a not-yet-loaded catalog must not force these to "unknown".
  assert.equal(resolveRuntimeProviderCapability("claude", false), "locked");
  assert.equal(resolveRuntimeProviderCapability("codex", false), "locked");
  assert.equal(resolveRuntimeProviderCapability(" claude ", false), "locked");
});

test("resolveRuntimeProviderCapability leaves genuinely unknown/custom runtimes as unknown", () => {
  // Preserves the tri-state's "omit rather than destructively write" behavior
  // for ids we can't statically classify (custom command, empty, unknown).
  assert.equal(resolveRuntimeProviderCapability("custom", false), "unknown");
  assert.equal(resolveRuntimeProviderCapability("", false), "unknown");
  assert.equal(
    resolveRuntimeProviderCapability("some-vendor-cli", false),
    "unknown",
  );
});
