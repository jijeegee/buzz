import assert from "node:assert/strict";
import test from "node:test";

import {
  EFFORT_DEFAULT_DROPDOWN_VALUE,
  EFFORT_DEFAULT_LABEL,
  EFFORT_FALLBACK_HELPER_TEXT,
  EFFORT_TEMPLATE_DEFAULT_LABEL,
  effortPickerState,
  effortSelectionToPersistedValue,
  effortSentinelLabel,
} from "./effortPicker.ts";

const localBackend = { type: "local" };
const providerBackend = { type: "provider", id: "openai", config: {} };
const options = [
  { value: "low", displayName: "Low" },
  { value: "high", displayName: "High" },
];
// What the Rust catalog publishes for Claude Code / Codex / Hermes
// (`effort_thought_level.fallback_values`) before any session has discovered
// the real list.
const fallback = ["low", "medium", "high"];

// ── Gating: discovered configId (pre-existing contract) ─────────────────────

test("effort picker renders for a local backend with a discovered configId", () => {
  const state = effortPickerState({
    backend: localBackend,
    effortConfigId: "thought_level",
    effortOptions: options,
    fallbackValues: null,
    currentEffort: null,
  });
  assert.equal(state.visible, true);
  assert.equal(state.usingFallback, false);
});

test("effort picker is hidden for a provider backend even when a configId exists", () => {
  const state = effortPickerState({
    backend: providerBackend,
    effortConfigId: "thought_level",
    effortOptions: options,
    fallbackValues: null,
    currentEffort: "high",
  });
  assert.equal(state.visible, false);
});

test("effort picker is hidden for a local backend with neither a configId nor a fallback", () => {
  for (const fallbackValues of [undefined, null, []]) {
    const state = effortPickerState({
      backend: localBackend,
      effortConfigId: undefined,
      effortOptions: undefined,
      fallbackValues,
      currentEffort: null,
    });
    assert.equal(
      state.visible,
      false,
      `no vocabulary (${JSON.stringify(fallbackValues)}) → nothing to pick`,
    );
  }
});

// ── Gating: catalog fallback before discovery ───────────────────────────────

test("effort picker is visible with catalog fallback options before any session has run", () => {
  // Pre-first-session / post-restart / post-runtime-switch: no configId, but
  // the prospective runtime's catalog entry publishes
  // effortThoughtLevel.fallbackValues.
  const state = effortPickerState({
    backend: localBackend,
    effortConfigId: undefined,
    effortOptions: undefined,
    fallbackValues: fallback,
    currentEffort: null,
  });
  assert.equal(state.visible, true);
  assert.equal(state.usingFallback, true);
  assert.deepEqual(state.options, [
    { label: "Adapter default", value: EFFORT_DEFAULT_DROPDOWN_VALUE },
    { label: "Low", value: "low" },
    { label: "Medium", value: "medium" },
    { label: "High", value: "high" },
  ]);
  assert.equal(state.selectValue, EFFORT_DEFAULT_DROPDOWN_VALUE);
});

test("the fallback never renders the write control for a provider backend", () => {
  // The v4 provider regression pin, extended: a catalog fallback must not
  // reopen the provider-backend door the Rust command slams shut.
  const state = effortPickerState({
    backend: providerBackend,
    effortConfigId: undefined,
    effortOptions: undefined,
    fallbackValues: fallback,
    currentEffort: null,
  });
  assert.equal(state.visible, false);
});

test("discovered options win over the catalog fallback once a session advertises them", () => {
  const state = effortPickerState({
    backend: localBackend,
    effortConfigId: "thought_level",
    effortOptions: options,
    fallbackValues: fallback,
    currentEffort: null,
  });
  assert.equal(state.usingFallback, false);
  assert.deepEqual(
    state.options.map((option) => option.value),
    [EFFORT_DEFAULT_DROPDOWN_VALUE, "low", "high"],
    "the adapter's real (model-dependent) list replaces the static subset",
  );
});

test("a discovered configId with no values still falls back to the catalog list", () => {
  const state = effortPickerState({
    backend: localBackend,
    effortConfigId: "thought_level",
    effortOptions: [],
    fallbackValues: fallback,
    currentEffort: null,
  });
  assert.equal(state.visible, true);
  assert.equal(state.usingFallback, true);
  assert.equal(state.options.length, 1 + fallback.length);
});

test("a saved effort outside the fallback list stays selectable under its raw name", () => {
  // `xhigh` picked from a past discovered session must not be misreported as
  // "Adapter default" after a restart drops the discovered list.
  const state = effortPickerState({
    backend: localBackend,
    effortConfigId: undefined,
    effortOptions: undefined,
    fallbackValues: fallback,
    currentEffort: "xhigh",
  });
  assert.equal(state.selectValue, "xhigh");
  assert.deepEqual(state.options.at(-1), { label: "xhigh", value: "xhigh" });
});

test("the fallback helper copy names the model dependence", () => {
  assert.equal(EFFORT_FALLBACK_HELPER_TEXT, "Options may vary by model.");
});

// ── Options + preselect (discovered) ────────────────────────────────────────

test("options lead with the adapter-default sentinel then adapter values", () => {
  const state = effortPickerState({
    backend: localBackend,
    effortConfigId: "thought_level",
    effortOptions: options,
    fallbackValues: null,
    currentEffort: null,
  });
  assert.deepEqual(state.options, [
    { label: "Adapter default", value: EFFORT_DEFAULT_DROPDOWN_VALUE },
    { label: "Low", value: "low" },
    { label: "High", value: "high" },
  ]);
});

test("option label falls back to the raw value when displayName is absent", () => {
  const state = effortPickerState({
    backend: localBackend,
    effortConfigId: "thought_level",
    effortOptions: [{ value: "medium" }],
    fallbackValues: null,
    currentEffort: null,
  });
  assert.deepEqual(state.options[1], { label: "medium", value: "medium" });
});

test("current effort preselects the matching option", () => {
  const state = effortPickerState({
    backend: localBackend,
    effortConfigId: "thought_level",
    effortOptions: options,
    fallbackValues: null,
    currentEffort: "high",
  });
  assert.equal(state.selectValue, "high");
});

test("an unknown current effort falls back to the adapter-default sentinel when options are discovered", () => {
  // The adapter advertised its full list, so a value outside it is not
  // something the adapter would accept — don't manufacture an option for it.
  const state = effortPickerState({
    backend: localBackend,
    effortConfigId: "thought_level",
    effortOptions: options,
    fallbackValues: null,
    currentEffort: "extreme",
  });
  assert.equal(state.selectValue, EFFORT_DEFAULT_DROPDOWN_VALUE);
  assert.equal(state.options.length, 3);
});

test("a null current effort selects the adapter-default sentinel", () => {
  const state = effortPickerState({
    backend: localBackend,
    effortConfigId: "thought_level",
    effortOptions: options,
    fallbackValues: null,
    currentEffort: null,
  });
  assert.equal(state.selectValue, EFFORT_DEFAULT_DROPDOWN_VALUE);
});

test("the sentinel selection persists as null (clear to adapter default)", () => {
  assert.equal(
    effortSelectionToPersistedValue(EFFORT_DEFAULT_DROPDOWN_VALUE),
    null,
  );
});

test("a concrete selection persists as its explicit effort level", () => {
  assert.equal(effortSelectionToPersistedValue("high"), "high");
});

// ── Sentinel label ──────────────────────────────────────────────────────────

test("the sentinel row reads 'Adapter default' unless the surface names the tier it falls back to", () => {
  const adapter = effortPickerState({
    backend: localBackend,
    effortConfigId: undefined,
    effortOptions: undefined,
    fallbackValues: fallback,
    currentEffort: null,
  });
  assert.equal(adapter.options[0].label, EFFORT_DEFAULT_LABEL);
  assert.equal(EFFORT_DEFAULT_LABEL, "Adapter default");

  // A linked instance clearing its own column inherits the template default,
  // so the row must not promise the adapter default.
  const linked = effortPickerState({
    backend: localBackend,
    effortConfigId: undefined,
    effortOptions: undefined,
    fallbackValues: fallback,
    currentEffort: null,
    defaultLabel: EFFORT_TEMPLATE_DEFAULT_LABEL,
  });
  assert.deepEqual(linked.options[0], {
    label: "Template default",
    value: EFFORT_DEFAULT_DROPDOWN_VALUE,
  });
  assert.equal(
    linked.selectValue,
    EFFORT_DEFAULT_DROPDOWN_VALUE,
    "the label is presentation only — the sentinel value (and its null persistence) is unchanged",
  );
});

test("effortSentinelLabel names the template tier only for a linked instance", () => {
  assert.equal(effortSentinelLabel(true), EFFORT_TEMPLATE_DEFAULT_LABEL);
  assert.equal(effortSentinelLabel(false), EFFORT_DEFAULT_LABEL);
});
