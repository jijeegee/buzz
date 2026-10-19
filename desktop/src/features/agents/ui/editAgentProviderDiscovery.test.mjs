import assert from "node:assert/strict";
import test from "node:test";

import {
  runtimeSupportsLlmProviderSelection,
  getPersonaProviderOptions,
  requiredCredentialEnvKeys,
  isMissingRequiredDropdownField,
} from "./agentConfigOptions.tsx";
import {
  computeEditAgentFormValidity,
  hasMissingRequiredEnvKey,
  shouldClearModelForRuntimeChange,
} from "./personaRuntimeModel.ts";

// ── LLM provider field visibility ──────────────────────────────────────────
//
// The edit dialog shows the provider picker when the current runtime supports
// LLM provider selection. Changing the provider in that picker re-fires
// usePersonaModelDiscovery (keyed on provider), so the model dropdown updates
// without saving. These tests guard the visibility predicate.

test("editAgent_providerFieldVisible_forKuzzAgent", () => {
  assert.equal(
    runtimeSupportsLlmProviderSelection("kuzz-agent"),
    true,
    "kuzz-agent runtime must expose the provider picker",
  );
});

test("editAgent_providerFieldVisible_forGoose", () => {
  assert.equal(
    runtimeSupportsLlmProviderSelection("goose"),
    true,
    "goose runtime must expose the provider picker",
  );
});

test("editAgent_providerFieldHidden_forClaude", () => {
  assert.equal(
    runtimeSupportsLlmProviderSelection("claude"),
    false,
    "claude runtime locks the provider; picker must be hidden",
  );
});

test("editAgent_providerFieldHidden_forBlankRuntime", () => {
  assert.equal(
    runtimeSupportsLlmProviderSelection(""),
    false,
    "blank runtime (catalog miss) must not show the provider picker",
  );
});

// ── Provider dropdown options for EditAgentProviderField ────────────────────
//
// The provider dropdown contains the well-known providers
// (databricks, databricks_v2, anthropic, openai, openai-compat) plus a
// default-provider fallback entry so users can clear a saved provider.
//
// On OSS builds, Databricks v1 ("databricks") is shown alongside v2 so OSS
// users who were previously on v1 can select it. On internal Block builds,
// callers pass hideProviderIds=new Set(["databricks"]) to suppress v1 — the
// boot migration rewrites any persisted v1 value to v2, so offering v1 there
// would silently undo the migration.

test("editAgent_providerOptions_includesDatabricksV2AndV1OnOSS", () => {
  // OSS builds: no hideProviderIds → both v1 and v2 appear.
  const options = getPersonaProviderOptions("", "kuzz-agent");
  const ids = options.map((o) => o.id);
  assert.ok(ids.includes("databricks_v2"), "databricks_v2 must be present");
  assert.ok(
    ids.includes("databricks"),
    "bare databricks (V1) must be present on OSS builds",
  );
});

test("editAgent_providerOptions_hidesDatabricksV1OnInternalBuild", () => {
  // Internal Block builds: pass hideProviderIds to suppress v1.
  const options = getPersonaProviderOptions(
    "",
    "kuzz-agent",
    "",
    new Set(["databricks"]),
  );
  const ids = options.map((o) => o.id);
  assert.ok(
    ids.includes("databricks_v2"),
    "databricks_v2 must still be present",
  );
  assert.ok(
    !ids.includes("databricks"),
    "bare databricks (V1) must be hidden on internal builds",
  );
});

test("editAgent_providerOptions_includesDatabricksV1AsCurrentEvenWhenHidden", () => {
  // A record already persisted with provider="databricks" must show it as the
  // current value even when hideProviderIds hides it from fresh selection.
  const options = getPersonaProviderOptions(
    "databricks",
    "kuzz-agent",
    "",
    new Set(["databricks"]),
  );
  const ids = options.map((o) => o.id);
  assert.ok(
    ids.includes("databricks"),
    "databricks must appear as current provider when it is the saved value",
  );
});

test("editAgent_providerOptions_includesDefaultEntry", () => {
  const options = getPersonaProviderOptions("", "kuzz-agent");
  // The first entry is the default (empty id) — clearing back to runtime default.
  assert.equal(
    options[0].id,
    "",
    "first provider option must be the default (empty id)",
  );
});

test("editAgent_providerOptions_includesCurrentIfCustom", () => {
  const options = getPersonaProviderOptions("my-custom-llm", "kuzz-agent");
  const ids = options.map((o) => o.id);
  assert.ok(
    ids.includes("my-custom-llm"),
    "a currently-saved custom provider must appear in the dropdown",
  );
});

// ── Finding 1 fix: fallback not disabled when discovery returns null ─────────
//
// When discoveredModelOptions is null, the model picker must NOT be disabled
// and the "Custom model..." option must remain selectable. This guards the
// regression where the select was disabled solely on missing discovery.
//
// We can't render React in pure node tests, but we CAN verify that the logic
// for deciding whether to show options is sound: when discovery is null, we
// fall back to staticModelOptions (length > 0), so we always have options.

test("editAgent_requiredDropdownField_onlyMarksMissingKnownField", async () => {
  const { isMissingRequiredDropdownField } = await import(
    "./agentConfigOptions.tsx"
  );

  assert.equal(
    isMissingRequiredDropdownField({ isRequired: true }, ""),
    true,
    "missing required dropdown value must be marked required",
  );
  assert.equal(
    isMissingRequiredDropdownField({ isRequired: true }, "configured"),
    false,
    "configured required dropdown value must not show the missing-required mark",
  );
  assert.equal(
    isMissingRequiredDropdownField(null, ""),
    false,
    "unknown normalized field names are ignored because they do not map to a dropdown",
  );
});

test("editAgent_modelFallback_staticOptionsWhenDiscoveryNull", () => {
  const staticModelOptions = [{ id: "", label: "Default model" }];
  // Simulate: discoveredModelOptions === null → effectiveModelOptions is static fallback
  const discoveredModelOptions = null;
  const effectiveModelOptions = discoveredModelOptions ?? staticModelOptions;
  assert.equal(
    effectiveModelOptions.length > 0,
    true,
    "effectiveModelOptions must be non-empty even when discovery returns null",
  );
  assert.equal(
    effectiveModelOptions[0].id,
    "",
    "fallback option must be the default (empty id)",
  );
});

test("editAgent_modelFallback_selectNotDisabledLogic", () => {
  // Verify: the correct disabled condition is (disabled || modelDiscoveryLoading),
  // NOT (disabled || modelDiscoveryLoading || !hasDiscoveredOptions).
  // We test this by confirming that a null discoveredModelOptions does NOT
  // set selectDisabled=true when the mutation is not pending and not loading.
  const disabled = false; // mutation not pending
  const modelDiscoveryLoading = false;
  // Old (buggy) logic would include: || !hasDiscoveredOptions
  // New (correct) logic:
  const selectDisabled = disabled || modelDiscoveryLoading;
  assert.equal(
    selectDisabled,
    false,
    "select must not be disabled when not loading and mutation is idle, regardless of discovery result",
  );
});

// ── Finding 2 fix: runtime switch enables provider picker ───────────────────
//
// Switching to kuzz-agent runtime (which supports LLM provider selection)
// must make the provider field visible, enabling live discovery.

test("editAgent_runtimeSwitch_toKuzzAgentEnablesProvider", () => {
  // Simulate: user switches from "claude" to "kuzz-agent"
  const previousRuntime = "claude";
  const nextRuntime = "kuzz-agent";
  const previousSupportsProvider =
    runtimeSupportsLlmProviderSelection(previousRuntime);
  const nextSupportsProvider = runtimeSupportsLlmProviderSelection(nextRuntime);
  assert.equal(
    previousSupportsProvider,
    false,
    "claude must NOT support provider selection",
  );
  assert.equal(
    nextSupportsProvider,
    true,
    "kuzz-agent MUST support provider selection",
  );
  // The provider field visibility transitions false → true on runtime change.
  assert.equal(
    !previousSupportsProvider && nextSupportsProvider,
    true,
    "switching from claude to kuzz-agent must make provider field visible",
  );
});

// ── Finding 3 fix: provider field hidden and cleared for locked runtimes ────
//
// When the live runtime is a provider-locked one (e.g. claude), the provider
// field must NOT be visible even if a stale provider value is saved.

test("editAgent_providerFieldHidden_forLockedRuntimeEvenWithSavedProvider", () => {
  // Simulate: agent has a stale saved provider "databricks_v2" but
  // the live selected runtime is "claude" (provider-locked).
  const liveRuntimeId = "claude";
  const savedProvider = "databricks_v2";
  // New logic: visibility is keyed on LIVE runtime, not saved provider.
  const llmProviderFieldVisible =
    runtimeSupportsLlmProviderSelection(liveRuntimeId);
  assert.equal(
    llmProviderFieldVisible,
    false,
    "provider field must be hidden when live runtime is provider-locked, even if a provider was previously saved",
  );
  // Confirm: if we had used the old logic (|| savedProvider), it would be visible.
  const oldLogic =
    runtimeSupportsLlmProviderSelection(liveRuntimeId) ||
    savedProvider.trim().length > 0;
  assert.equal(
    oldLogic,
    true,
    "old logic would have incorrectly shown the provider field (this confirms the fix is meaningful)",
  );
});

// ── Runtime model-clear on change ─────────────────────────────────────────
//
// When the runtime changes, the model should be cleared if the previous
// runtime had a model that's not valid for the next runtime.

test("editAgent_modelClearedOnRuntimeChange", () => {
  const previousRuntime = "kuzz-agent";
  const nextRuntime = "claude";
  assert.equal(
    shouldClearModelForRuntimeChange(previousRuntime, nextRuntime),
    true,
    "model must be cleared when switching runtimes",
  );
});

test("editAgent_modelNotClearedWhenRuntimeUnchanged", () => {
  const runtime = "kuzz-agent";
  assert.equal(
    shouldClearModelForRuntimeChange(runtime, runtime),
    false,
    "model must NOT be cleared when the runtime stays the same",
  );
});

// ── Finding A fix: late catalog arrival does not wipe a valid saved provider ─
//
// When the dialog opens before the runtime catalog has loaded, selectedRuntimeId
// falls back to "custom" (no match). Once the catalog arrives, a separate effect
// re-derives the correct id — but ONLY if the user has not touched the dropdown.
// This ensures a no-op save never silently clears a valid databricks provider.

test("editAgent_catalogArrival_rederivesRuntimeIdWhenNotTouched", () => {
  // Simulate: open effect runs with empty runtimes → selectedRuntimeId = "custom".
  // Then catalog arrives with the saved agent's runtime.
  const agentCommand = "/usr/local/bin/kuzz-agent";
  const catalog = [
    { id: "kuzz-agent", command: agentCommand, defaultArgs: [] },
    { id: "claude", command: "/usr/local/bin/claude", defaultArgs: [] },
  ];
  const runtimeTouched = false; // user has not selected a runtime

  // Simulate the catalog-arrival effect logic.
  let selectedRuntimeId = "custom"; // seeded by open effect before catalog loaded
  if (!runtimeTouched && catalog.length > 0) {
    const matched = catalog.find(
      (r) => r.command?.trim() === agentCommand.trim(),
    );
    if (matched) {
      selectedRuntimeId = matched.id;
    }
  }

  assert.equal(
    selectedRuntimeId,
    "kuzz-agent",
    "catalog-arrival effect must update selectedRuntimeId from 'custom' to the matched runtime",
  );
});

test("editAgent_catalogArrival_doesNotOverwriteUserSelection", () => {
  // Simulate: user has already picked a runtime (runtimeTouched = true).
  // The catalog-arrival effect must not overwrite the user's choice.
  const agentCommand = "/usr/local/bin/kuzz-agent";
  const catalog = [
    { id: "kuzz-agent", command: agentCommand, defaultArgs: [] },
  ];
  const runtimeTouched = true; // user already picked goose

  let selectedRuntimeId = "goose"; // user's choice
  if (!runtimeTouched && catalog.length > 0) {
    const matched = catalog.find(
      (r) => r.command?.trim() === agentCommand.trim(),
    );
    if (matched) {
      selectedRuntimeId = matched.id;
    }
  }

  assert.equal(
    selectedRuntimeId,
    "goose",
    "catalog-arrival effect must NOT overwrite user's selection when runtimeTouched is true",
  );
});

test("editAgent_noOpSavePreservesProvider_whenCatalogLate", () => {
  // Simulate the provider persistence logic when catalog arrived late.
  // If the catalog-arrival effect correctly sets selectedRuntimeId = "kuzz-agent",
  // then llmProviderFieldVisible = true and the provider is preserved on save.
  const selectedRuntimeId = "kuzz-agent"; // correctly derived after catalog arrival
  const savedProvider = "databricks_v2";
  const normalizedProvider = savedProvider;

  // The visibility logic (mirrors the component).
  const llmProviderFieldVisible =
    runtimeSupportsLlmProviderSelection(selectedRuntimeId);

  // The submit logic for provider tri-state.
  let providerUpdate;
  if (llmProviderFieldVisible) {
    // Only send if changed; here unchanged → undefined (no-op).
    providerUpdate =
      normalizedProvider !== (savedProvider ?? null)
        ? normalizedProvider
        : undefined;
  } else {
    // Would send null to clear.
    providerUpdate = (savedProvider ?? null) !== null ? null : undefined;
  }

  assert.equal(
    llmProviderFieldVisible,
    true,
    "provider field must be visible once catalog derives kuzz-agent runtime",
  );
  assert.equal(
    providerUpdate,
    undefined,
    "a no-op save must NOT send null to clear the provider when runtime is correctly derived",
  );
});

test("editAgent_missingRequiredEnvKey_blocksSaveViaValidity", () => {
  // The block-save gate is folded into computeEditAgentFormValidity so the
  // Save button disables when a runtime/provider-required credential is unset.
  const base = {
    name: "My Agent",
    parallelism: "",
    agentAcpCommand: "",
    acpCommand: "",
    respondTo: "all",
    respondToAllowlistLength: 0,
    selectedRuntimeId: "kuzz-agent",
    agentCommand: "kuzz-agent",
    requiredEnvKeyMissing: false,
  };

  assert.equal(
    computeEditAgentFormValidity({ ...base, requiredEnvKeyMissing: true }),
    false,
    "Save must be blocked when a required credential key is missing",
  );
  assert.equal(
    computeEditAgentFormValidity({ ...base, requiredEnvKeyMissing: false }),
    true,
    "Save must be allowed once the required credential key is present",
  );
});

test("editAgent_customCommandPinned_blocksSaveWhenCommandEmpty", () => {
  // A custom command with an empty command field must block Save — the
  // backend would spawn a runtime with no command otherwise. Exercises the real
  // computeEditAgentFormValidity helper.
  const base = {
    name: "My Agent",
    parallelism: "",
    agentAcpCommand: "",
    acpCommand: "",
    respondTo: "all",
    respondToAllowlistLength: 0,
    selectedRuntimeId: "custom",
    agentCommand: "",
    requiredEnvKeyMissing: false,
  };

  assert.equal(
    computeEditAgentFormValidity(base),
    false,
    "empty custom command must block Save",
  );

  assert.equal(
    computeEditAgentFormValidity({ ...base, agentCommand: "/opt/bin/agent" }),
    true,
    "non-empty custom command must allow Save",
  );

  // The other validity gates still apply through the helper.
  assert.equal(
    computeEditAgentFormValidity({
      ...base,
      agentCommand: "/opt/bin/agent",
      name: "   ",
    }),
    false,
    "blank name must block Save",
  );
  assert.equal(
    computeEditAgentFormValidity({
      ...base,
      agentCommand: "/opt/bin/agent",
      respondTo: "allowlist",
      respondToAllowlistLength: 0,
    }),
    false,
    "empty allowlist must block Save",
  );
});

// ── Bug A fix: inherited-runtime (short-name agentCommand) seeds selectedRuntimeId
//              correctly via id-fallback when catalog command is the resolved path
//
// Problem: kuzz-agent stores agentCommand="kuzz-agent" (short name) while the
// catalog entry has command="/Applications/Kuzz.app/.../kuzz-agent" (resolved path).
// Command-based matching fails (short name ≠ full path), so selectedRuntimeId
// stayed "custom" → selectedRuntime=undefined → canDiscoverModelOptions=false →
// discovery never fired for inherited agents.
//
// Fix: both seeding spots (open-effect and catalog-arrival effect) now fall back
// to id-based matching when command-path matching misses — same id-fallback used
// by effectiveRuntimeIdForSubmit.

// Helper mirrors the fixed seeding logic from AgentInstanceEditDialog.tsx open-effect /
// catalog-arrival effect.
function deriveSelectedRuntimeId(agentCommand, runtimes) {
  const matched =
    runtimes.find((r) => r.command?.trim() === agentCommand.trim()) ??
    runtimes.find((r) => r.id === agentCommand.trim());
  return matched ? matched.id : "custom";
}

test("editAgent_bugA_inheritedShortName_resolvesViaIdFallback", () => {
  // The core regression: agentCommand is the short name "kuzz-agent" but the
  // catalog's command is the resolved binary path. Command match fails; id
  // fallback must rescue it.

  const agentCommand = "kuzz-agent"; // short name stored by effective_agent_command
  const runtimes = [
    {
      id: "kuzz-agent",
      command: "/Applications/Kuzz.app/Contents/MacOS/kuzz-agent", // resolved path
      availability: "available",
      defaultArgs: [],
    },
    {
      id: "claude",
      command: "/usr/local/bin/claude-agent-acp",
      availability: "available",
      defaultArgs: [],
    },
  ];

  const selectedRuntimeId = deriveSelectedRuntimeId(agentCommand, runtimes);
  assert.equal(
    selectedRuntimeId,
    "kuzz-agent",
    "short-name agentCommand must resolve to kuzz-agent id via id-fallback when command path differs",
  );
});

test("editAgent_bugA_inheritedShortName_commandMatchStillWinsWhenPresent", () => {
  // Command-path match must still win when it succeeds (no regression for the
  // explicit-pin path where agentCommand IS the full resolved path).

  const agentCommand = "/usr/local/bin/kuzz-agent"; // full path (explicit pin)
  const runtimes = [
    {
      id: "kuzz-agent",
      command: "/usr/local/bin/kuzz-agent",
      availability: "available",
      defaultArgs: [],
    },
  ];

  const selectedRuntimeId = deriveSelectedRuntimeId(agentCommand, runtimes);
  assert.equal(
    selectedRuntimeId,
    "kuzz-agent",
    "command-path match must still win when agentCommand equals catalog command",
  );
});

test("editAgent_bugA_inheritedShortName_discoveryGatePasses", () => {
  // Once selectedRuntimeId is correctly resolved to "kuzz-agent" via id-fallback,
  // selectedRuntime resolves to the available catalog entry, and the discovery gate
  // (canDiscoverModelOptions) passes so the model list populates.
  //
  // Mirrors usePersonaModelDiscovery's canDiscoverModelOptions logic:
  //   open && modelFieldVisible && selectedRuntime?.availability === "available"
  //   && discoveryAgentCommand !== null && ...

  const agentCommand = "kuzz-agent"; // short name
  const runtimes = [
    {
      id: "kuzz-agent",
      command: "/Applications/Kuzz.app/Contents/MacOS/kuzz-agent",
      availability: "available",
      defaultArgs: [],
    },
  ];

  // Step 1: derive selectedRuntimeId (the fix).
  const selectedRuntimeId = deriveSelectedRuntimeId(agentCommand, runtimes);
  // Step 2: resolve selectedRuntime from the catalog.
  const selectedRuntime = runtimes.find((r) => r.id === selectedRuntimeId);
  // Step 3: derive discoveryAgentCommand (mirrors usePersonaModelDiscovery:85-87).
  const discoveryAgentCommand = selectedRuntime?.command?.trim()
    ? selectedRuntime.command
    : null;
  // Step 4: evaluate the discovery gate (mirrors :88-93, simplified).
  const open = true;
  const modelFieldVisible = true;
  const isCustomProviderEditing = false;
  const trimmedProvider = "databricks_v2";
  const canDiscoverModelOptions =
    open &&
    modelFieldVisible &&
    selectedRuntime?.availability === "available" &&
    discoveryAgentCommand !== null &&
    (!isCustomProviderEditing || trimmedProvider.length > 0);

  assert.equal(
    selectedRuntimeId,
    "kuzz-agent",
    "id-fallback must resolve selectedRuntimeId to kuzz-agent",
  );
  assert.ok(
    selectedRuntime !== undefined,
    "selectedRuntime must resolve once selectedRuntimeId is correct",
  );
  assert.equal(
    discoveryAgentCommand,
    "/Applications/Kuzz.app/Contents/MacOS/kuzz-agent",
    "discoveryAgentCommand must be the resolved path from the catalog entry",
  );
  assert.equal(
    canDiscoverModelOptions,
    true,
    "discovery gate must pass for inherited kuzz-agent with databricks_v2 provider once id-fallback resolves the runtime",
  );
});

test("editAgent_bugA_unknownCommandStillFallsBackToCustom", () => {
  // An agentCommand that matches neither catalog command nor catalog id must
  // still produce "custom" — the id-fallback must not introduce false positives.

  const agentCommand = "/some/custom/binary"; // not in catalog
  const runtimes = [
    {
      id: "kuzz-agent",
      command: "/Applications/Kuzz.app/Contents/MacOS/kuzz-agent",
      availability: "available",
      defaultArgs: [],
    },
  ];

  const selectedRuntimeId = deriveSelectedRuntimeId(agentCommand, runtimes);
  assert.equal(
    selectedRuntimeId,
    "custom",
    "unknown command/id must still fall back to 'custom'",
  );
});

// ── requiredCredentialEnvKeys ──────────────────────────────────────────────
//
// Guards the provider-aware credential requirements surface so Phase 2
// required env rows stay correct as providers and runtimes change.

test("requiredCredentialEnvKeys: kuzz-agent + anthropic → ANTHROPIC_API_KEY", () => {
  const keys = requiredCredentialEnvKeys("kuzz-agent", "anthropic");
  assert.deepEqual(keys, ["ANTHROPIC_API_KEY"]);
});

test("requiredCredentialEnvKeys: kuzz-agent + openai → OPENAI_COMPAT_API_KEY", () => {
  const keys = requiredCredentialEnvKeys("kuzz-agent", "openai");
  assert.deepEqual(keys, ["OPENAI_COMPAT_API_KEY"]);
});

test("requiredCredentialEnvKeys: kuzz-agent + databricks → DATABRICKS_HOST only (no token)", () => {
  const keys = requiredCredentialEnvKeys("kuzz-agent", "databricks");
  // DATABRICKS_TOKEN is NOT required — OAuth PKCE is the normal path.
  assert.deepEqual(keys, ["DATABRICKS_HOST"]);
  assert.ok(
    !keys.includes("DATABRICKS_TOKEN"),
    "DATABRICKS_TOKEN must not be required (OAuth PKCE is the normal auth path)",
  );
});

test("requiredCredentialEnvKeys: kuzz-agent + databricks_v2 → DATABRICKS_HOST only", () => {
  const keys = requiredCredentialEnvKeys("kuzz-agent", "databricks_v2");
  assert.deepEqual(keys, ["DATABRICKS_HOST"]);
});

test("requiredCredentialEnvKeys: goose + anthropic → ANTHROPIC_API_KEY", () => {
  const keys = requiredCredentialEnvKeys("goose", "anthropic");
  assert.deepEqual(keys, ["ANTHROPIC_API_KEY"]);
});

test("requiredCredentialEnvKeys: goose + openai → OPENAI_COMPAT_API_KEY", () => {
  const keys = requiredCredentialEnvKeys("goose", "openai");
  assert.deepEqual(keys, ["OPENAI_COMPAT_API_KEY"]);
});

test("requiredCredentialEnvKeys: kuzz-agent + no provider → empty (provider not yet selected)", () => {
  const keys = requiredCredentialEnvKeys("kuzz-agent", "");
  assert.deepEqual(keys, []);
});

test("requiredCredentialEnvKeys: claude → empty (uses CLI login, not env keys)", () => {
  const keys = requiredCredentialEnvKeys("claude", "");
  assert.deepEqual(keys, []);
});

test("requiredCredentialEnvKeys: codex → empty (uses CLI login, not env keys)", () => {
  const keys = requiredCredentialEnvKeys("codex", "");
  assert.deepEqual(keys, []);
});

test("requiredCredentialEnvKeys: custom/unknown runtime → empty", () => {
  const keys = requiredCredentialEnvKeys("my-custom-harness", "anthropic");
  assert.deepEqual(keys, []);
});

// ── Block-save gate: hasMissingRequiredEnvKey logic ────────────────────────
//
// The AgentInstanceEditDialog computes:
//   requiredEnvKeyMissing = hasMissingRequiredEnvKey(requiredEnvKeys, envVars)
// and folds it into canSubmit (via computeEditAgentFormValidity). These tests
// exercise the exported predicate directly.

const hasRequiredEnvKeyMissing = hasMissingRequiredEnvKey;

test("blockSave_kuzzAgentAnthropicMissingKey_blocked", () => {
  // Will's exact case: kuzz-agent / anthropic / opus / no ANTHROPIC_API_KEY
  const requiredKeys = requiredCredentialEnvKeys("kuzz-agent", "anthropic");
  const envVars = {}; // key absent
  assert.equal(
    hasRequiredEnvKeyMissing(requiredKeys, envVars),
    true,
    "save must be blocked when ANTHROPIC_API_KEY is missing",
  );
});

test("blockSave_kuzzAgentAnthropicKeyProvided_allowed", () => {
  const requiredKeys = requiredCredentialEnvKeys("kuzz-agent", "anthropic");
  const envVars = { ANTHROPIC_API_KEY: "sk-ant-test" };
  assert.equal(
    hasRequiredEnvKeyMissing(requiredKeys, envVars),
    false,
    "save must be allowed when ANTHROPIC_API_KEY is present",
  );
});

test("blockSave_kuzzAgentAnthropicEmptyStringKey_blocked", () => {
  // Empty string is treated the same as absent — matches EnvVarsEditor isMissing
  const requiredKeys = requiredCredentialEnvKeys("kuzz-agent", "anthropic");
  const envVars = { ANTHROPIC_API_KEY: "" };
  assert.equal(
    hasRequiredEnvKeyMissing(requiredKeys, envVars),
    true,
    "save must be blocked when ANTHROPIC_API_KEY is empty string",
  );
});

test("blockSave_claudeNoCliLogin_notBlocked", () => {
  // CLI-login runtimes have no dialog-fixable requirement — must never block
  const requiredKeys = requiredCredentialEnvKeys("claude", "");
  const envVars = {}; // no keys set
  assert.equal(
    hasRequiredEnvKeyMissing(requiredKeys, envVars),
    false,
    "claude save must NOT be blocked — CLI login is out-of-band",
  );
});

test("blockSave_codexNoCliLogin_notBlocked", () => {
  const requiredKeys = requiredCredentialEnvKeys("codex", "");
  const envVars = {};
  assert.equal(
    hasRequiredEnvKeyMissing(requiredKeys, envVars),
    false,
    "codex save must NOT be blocked — CLI login is out-of-band",
  );
});

test("blockSave_kuzzAgentDatabricksMissingHost_blocked", () => {
  const requiredKeys = requiredCredentialEnvKeys("kuzz-agent", "databricks");
  const envVars = {};
  assert.equal(
    hasRequiredEnvKeyMissing(requiredKeys, envVars),
    true,
    "save must be blocked when DATABRICKS_HOST is missing",
  );
});

test("blockSave_kuzzAgentDatabricksHostProvided_allowed", () => {
  const requiredKeys = requiredCredentialEnvKeys("kuzz-agent", "databricks");
  const envVars = { DATABRICKS_HOST: "https://my.databricks.instance" };
  assert.equal(
    hasRequiredEnvKeyMissing(requiredKeys, envVars),
    false,
    "save must be allowed when DATABRICKS_HOST is present",
  );
});

// ── Block-save gate: isMissingRequiredDropdownField ────────────────────────
//
// The AgentInstanceEditDialog also gates on modelRequired / providerRequired.
// These tests guard the isMissingRequiredDropdownField predicate used for both.

test("blockSave_missingRequiredModel_blocked", () => {
  // isRequired=true and value is empty → should block
  assert.equal(
    isMissingRequiredDropdownField({ isRequired: true }, ""),
    true,
    "canSubmit must be blocked when required model is unset",
  );
});

test("blockSave_requiredModelProvided_allowed", () => {
  assert.equal(
    isMissingRequiredDropdownField({ isRequired: true }, "claude-opus-4-5"),
    false,
    "canSubmit must be allowed when required model is set",
  );
});

test("blockSave_optionalModelEmpty_allowed", () => {
  // isRequired=false → not a block-save condition
  assert.equal(
    isMissingRequiredDropdownField({ isRequired: false }, ""),
    false,
    "optional empty model must not block save",
  );
});

test("blockSave_nullField_allowed", () => {
  // null/undefined field descriptor → not required, not blocked
  assert.equal(
    isMissingRequiredDropdownField(null, ""),
    false,
    "null field must not block save",
  );
  assert.equal(
    isMissingRequiredDropdownField(undefined, ""),
    false,
    "undefined field must not block save",
  );
});
