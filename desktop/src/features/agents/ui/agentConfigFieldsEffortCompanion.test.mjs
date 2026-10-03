/**
 * Mounted gating pins for the Global AI Defaults effort companion.
 *
 * `AgentConfigFields` (Agent defaults dialog, settings card, onboarding) lets
 * the user pick a default model for any harness. For ACP thought-level
 * harnesses (Claude Code, Codex, Hermes) there is no native env knob, so the
 * global default is the structured `GlobalAgentConfig.effort_level` column,
 * rendered beneath the model control through the shared
 * `ModelEffortFields`/`effortPickerState` path. These tests mount the real
 * component with a Claude catalog entry and pin:
 *   - the companion renders beside the model and a pick writes
 *     `effort_level` on the config (not an env var);
 *   - the sentinel clears it back to null;
 *   - a native-knob harness (Goose) keeps its env-var EffortSelectField and
 *     never renders the column companion.
 */

import assert from "node:assert/strict";
import { afterEach, before, test } from "node:test";
import { JSDOM } from "jsdom";

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://localhost",
});

Object.assign(globalThis, {
  document: dom.window.document,
  window: dom.window,
  IS_REACT_ACT_ENVIRONMENT: true,
  localStorage: dom.window.localStorage,
  self: dom.window,
  ResizeObserver: class {
    observe() {}
    unobserve() {}
    disconnect() {}
  },
});
Object.defineProperty(globalThis, "navigator", {
  configurable: true,
  value: dom.window.navigator,
  writable: true,
});
dom.window.requestAnimationFrame = (cb) => setTimeout(cb, 0);
globalThis.requestAnimationFrame = dom.window.requestAnimationFrame;
dom.window.matchMedia ??= (query) => ({
  matches: false,
  media: query,
  onchange: null,
  addListener: () => {},
  removeListener: () => {},
  addEventListener: () => {},
  removeEventListener: () => {},
  dispatchEvent: () => false,
});
globalThis.matchMedia = dom.window.matchMedia;
for (const key of Object.getOwnPropertyNames(dom.window)) {
  if (key === "window" || key === "document" || key === "globalThis") continue;
  const value = dom.window[key];
  if (
    typeof value === "function" &&
    /^(HTML|SVG)|Element$|Event$|EventTarget$|^Node|^Document|Observer$/.test(
      key,
    )
  ) {
    globalThis[key] = value;
  }
}
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
dom.window.HTMLElement.prototype.hasPointerCapture = () => false;
dom.window.HTMLElement.prototype.releasePointerCapture = () => {};
dom.window.HTMLElement.prototype.scrollIntoView = () => {};
const _origDispatch = dom.window.EventTarget.prototype.dispatchEvent;
dom.window.EventTarget.prototype.dispatchEvent = function (event) {
  if (!(event instanceof dom.window.Event)) return false;
  return _origDispatch.call(this, event);
};
globalThis.EventTarget = dom.window.EventTarget;

globalThis.__TAURI_INTERNALS__ = {
  invoke: (cmd) => {
    if (cmd === "discover_agent_models")
      return Promise.resolve({
        agentName: "claude",
        agentVersion: "1.0",
        models: [],
        agentDefaultModel: null,
        selectedModel: null,
        supportsSwitching: false,
      });
    return Promise.reject(new Error(`unmocked: ${cmd}`));
  },
  transformCallback: () => 1,
};
dom.window.__TAURI_INTERNALS__ = globalThis.__TAURI_INTERNALS__;

let act;
let cleanup;
let fireEvent;
let render;
let createElement;
let AgentConfigFields;
let fromRawAcpRuntimeCatalogEntry;

before(async () => {
  ({ act, cleanup, fireEvent, render } = await import(
    "@testing-library/react"
  ));
  ({ createElement } = await import("react"));
  ({ AgentConfigFields } = await import("./AgentConfigFields.tsx"));
  ({ fromRawAcpRuntimeCatalogEntry } = await import(
    "../../../shared/api/tauri.ts"
  ));
});

afterEach(() => {
  cleanup?.();
});

function rawRuntime(id, overrides = {}) {
  return {
    id,
    label: id,
    avatar_url: "",
    availability: "available",
    command: `${id}-cmd`,
    binary_path: `/usr/local/bin/${id}`,
    default_args: [],
    mcp_command: null,
    model_env_var: null,
    provider_env_var: null,
    thinking_env_var: null,
    max_tokens_env_var: null,
    context_limit_env_var: null,
    max_rounds_env_var: null,
    install_hint: "",
    install_instructions_url: "",
    can_auto_install: false,
    requires_external_cli: false,
    underlying_cli_path: null,
    node_required: false,
    auth_status: { status: "logged_in" },
    login_hint: null,
    source: "builtin",
    ...overrides,
  };
}

const claude = () =>
  fromRawAcpRuntimeCatalogEntry(
    rawRuntime("claude", {
      effort_thought_level: {
        config_option_id: "effort",
        fallback_values: ["low", "medium", "high"],
      },
    }),
  );
const goose = () =>
  fromRawAcpRuntimeCatalogEntry(
    rawRuntime("goose", {
      model_env_var: "GOOSE_MODEL",
      provider_env_var: "GOOSE_PROVIDER",
      thinking_env_var: "GOOSE_THINKING_EFFORT",
      effort_canonical_values: ["off", "low", "medium", "high", "max"],
    }),
  );

async function mount(runtime, config, onConfigChange = () => {}) {
  await act(async () => {
    render(
      createElement(AgentConfigFields, {
        bakedEnv: [],
        selectedRuntime: runtime,
        config,
        isCustomModelEditing: false,
        isCustomProvider: false,
        onConfigChange,
        onCustomModelEditingChange: () => {},
        onIsCustomProviderChange: () => {},
        disclosure: "full",
        useCustomSelect: true,
      }),
    );
  });
  await act(async () => {});
  const doc = dom.window.document;
  return {
    model: doc.getElementById("global-agent-model"),
    column: doc.getElementById("global-agent-effort"),
    envKnob: doc.querySelector(
      '[data-testid="global-agent-thinking-effort-select"]',
    ),
  };
}

async function pick(trigger, label) {
  await act(async () => {
    fireEvent.pointerDown(
      trigger,
      new dom.window.MouseEvent("pointerdown", { bubbles: true, button: 0 }),
    );
    fireEvent.click(trigger);
  });
  const item = [
    ...dom.window.document.querySelectorAll('[role="menuitemradio"]'),
  ].find((node) => node.textContent?.trim() === label);
  assert.ok(item, `option "${label}" must be offered`);
  await act(async () => {
    fireEvent.click(item);
  });
}

test("a thought-level harness gets the global effort column beside its default model", async () => {
  const changes = [];
  const { model, column, envKnob } = await mount(
    claude(),
    {
      env_vars: {},
      provider: null,
      model: "opus",
      preferred_runtime: "claude",
    },
    (next) => changes.push(next),
  );
  assert.ok(model, "the default-model control renders");
  assert.ok(
    column,
    "Claude Code has no env knob, so its default is the column",
  );
  assert.equal(
    envKnob,
    null,
    "the env-var EffortSelectField is not for Claude",
  );
  assert.ok(
    column.closest('[data-testid="model-effort-fields"]'),
    "the column renders as the model control's companion",
  );

  await pick(column, "High");
  assert.equal(changes.length, 1);
  assert.equal(
    changes[0].effort_level,
    "high",
    "a pick writes the structured column",
  );
  assert.deepEqual(changes[0].env_vars, {}, "…never an env var");
});

test("the stored global effort preselects and the sentinel clears it to null", async () => {
  const changes = [];
  const { column } = await mount(
    claude(),
    {
      env_vars: {},
      provider: null,
      model: "opus",
      preferred_runtime: "claude",
      effort_level: "medium",
    },
    (next) => changes.push(next),
  );
  assert.ok(column.textContent?.includes("Medium"), column.textContent);

  await pick(column, "Adapter default");
  assert.equal(
    changes.at(-1).effort_level,
    null,
    "null = adapter default, like the dialogs",
  );
});

test("a native-knob harness keeps its env-var effort control and never renders the column", async () => {
  const { column, envKnob } = await mount(goose(), {
    env_vars: { GOOSE_THINKING_EFFORT: "off" },
    provider: "anthropic",
    model: "claude-3-5-sonnet",
    preferred_runtime: "goose",
  });
  assert.equal(column, null, "Goose publishes no effortThoughtLevel");
  assert.ok(envKnob, "the GOOSE_THINKING_EFFORT control is unchanged");
});
