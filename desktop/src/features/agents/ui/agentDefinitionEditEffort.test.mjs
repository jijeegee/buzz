/**
 * Definition-edit effort acceptance pins (production seam).
 *
 * The owner's "Edit" from the Agents library opens `AgentDialog` in
 * `definition-edit` mode, i.e. `AgentDefinitionDialog` on a stored persona.
 * Before this change that mode hid the effort picker (`isCreateMode` gate), so
 * an agent created with an effort could never show or change it. These tests
 * mount the real `AgentDialog` in that mode, seed it through the real
 * `editPersonaDialogState`, drive the REAL dropdowns and the REAL
 * "Save changes" button, and assert the `UpdatePersonaInput` that reaches the
 * caller's `onSubmit`:
 *   - the picker is offered beside the model control and preselects the
 *     stored definition effort;
 *   - an untouched save round-trips that effort (no silent clear);
 *   - picking the sentinel submits `effortLevel: null` (an explicit clear);
 *   - switching to a harness that publishes no effort vocabulary hides the
 *     picker and submits `null`, so a stale pick never survives onto it.
 */

import assert from "node:assert/strict";
import { after, afterEach, before, test } from "node:test";

import { JSDOM } from "jsdom";

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://localhost",
});

const clients = [];

let act;
let cleanup;
let fireEvent;
let render;
let screen;
let createElement;
let QueryClient;
let QueryClientProvider;
let ThemeProvider;
let AgentDialog;
let editPersonaDialogState;
let fromRawAcpRuntimeCatalogEntry;

const ipcHandlers = new Map();
const unmocked = [];
const OWNER_PK = "a".repeat(64);

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
    install_hint: "",
    install_instructions_url: "",
    can_auto_install: false,
    underlying_cli_path: null,
    node_required: false,
    auth_status: { status: "logged_in" },
    source: "builtin",
    ...overrides,
  };
}

// Claude Code publishes its pre-discovery effort vocabulary from the Rust
// catalog; "cursor" stands in for a harness that publishes none.
const CLAUDE_THOUGHT_LEVEL = {
  config_option_id: "effort",
  fallback_values: ["low", "medium", "high"],
};

/** A stored definition like the owner's: Claude Code, explicit model, effort set. */
function storedPersona(overrides = {}) {
  return {
    id: "p-1",
    displayName: "집클로드",
    avatarUrl: null,
    description: null,
    systemPrompt: "You help.",
    acpCommand: "buzz-acp",
    runtime: "claude",
    model: "fable[1m]",
    provider: null,
    effortLevel: "high",
    namePool: [],
    isBuiltIn: false,
    isActive: true,
    shared: false,
    sourceTeam: null,
    catalogSource: null,
    envVars: {},
    respondTo: null,
    respondToAllowlist: [],
    parallelism: null,
    sessionPolicy: "channel",
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

function installIpc() {
  const set = (cmd, handler) => ipcHandlers.set(cmd, handler);
  set("get_identity", () =>
    Promise.resolve({ pubkey: OWNER_PK, npub: `npub1${"a".repeat(58)}` }),
  );
  set("get_global_agent_config", () =>
    Promise.resolve({
      env_vars: {},
      provider: null,
      model: null,
      preferred_runtime: "claude",
    }),
  );
  set("get_baked_build_env", () => Promise.resolve([]));
  set("get_baked_build_env_keys", () => Promise.resolve([]));
  set("get_runtime_file_config", () => Promise.resolve(null));
  set("discover_acp_commands", () => Promise.resolve([]));
  set("discover_acp_providers", () => Promise.resolve([]));
  set("discover_backend_providers", () => Promise.resolve([]));
  set("discover_agent_models", () =>
    Promise.resolve({
      agentName: "claude",
      agentVersion: "1.0",
      models: [],
      agentDefaultModel: null,
      selectedModel: null,
      supportsSwitching: false,
    }),
  );
}

function runtimes() {
  return [
    rawRuntime("claude", { effort_thought_level: CLAUDE_THOUGHT_LEVEL }),
    rawRuntime("cursor"),
  ].map(fromRawAcpRuntimeCatalogEntry);
}

async function settle(ms = 20) {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, ms));
  });
}

async function openEditDialog(persona = storedPersona()) {
  const submitted = [];
  const client = new QueryClient({
    defaultOptions: {
      mutations: { gcTime: 0 },
      queries: { gcTime: 0, retry: false },
    },
  });
  clients.push(client);
  const state = editPersonaDialogState(persona);
  await act(async () => {
    render(
      createElement(
        ThemeProvider,
        { defaultTheme: "buzz" },
        createElement(
          QueryClientProvider,
          { client },
          createElement(AgentDialog, {
            description: state.description,
            error: null,
            initialValues: state.initialValues,
            isPending: false,
            mode: "definition-edit",
            onOpenChange: () => {},
            onSubmit: async (input) => {
              submitted.push(input);
            },
            open: true,
            runtimes: runtimes(),
            runtimeCatalogStatus: "ready",
            submitLabel: state.submitLabel,
            title: state.title,
          }),
        ),
      ),
    );
  });
  await settle(50);
  assert.ok(
    dom.window.document.getElementById("persona-display-name"),
    "Edit dialog must open with the display-name field",
  );
  return submitted;
}

async function pickFromDropdown(triggerId, matchLabel) {
  const trigger = dom.window.document.getElementById(triggerId);
  assert.ok(trigger, `dropdown trigger #${triggerId} must render`);
  await act(async () => {
    fireEvent.pointerDown(
      trigger,
      new dom.window.MouseEvent("pointerdown", { bubbles: true, button: 0 }),
    );
    fireEvent.click(trigger);
  });
  const item = [
    ...dom.window.document.querySelectorAll('[role="menuitemradio"]'),
  ].find((node) => matchLabel(node.textContent?.trim() ?? ""));
  assert.ok(item, `an option matching the requested label must be offered`);
  await act(async () => {
    fireEvent.click(item);
  });
  await settle();
}

async function save(submitted) {
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  });
  for (let i = 0; i < 50 && submitted.length === 0; i += 1) {
    await settle(20);
  }
  assert.equal(
    submitted.length,
    1,
    `Save must submit exactly once (unmocked: ${unmocked.join(", ") || "none"})`,
  );
  return submitted[0];
}

before(async () => {
  Object.assign(globalThis, {
    document: dom.window.document,
    window: dom.window,
    IS_REACT_ACT_ENVIRONMENT: true,
    localStorage: dom.window.localStorage,
  });
  for (const key of Object.getOwnPropertyNames(dom.window)) {
    if (key === "window" || key === "document" || key === "globalThis")
      continue;
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
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: dom.window.navigator,
    writable: true,
  });
  dom.window.matchMedia = () => ({
    matches: true,
    addEventListener() {},
    removeEventListener() {},
  });
  dom.window.HTMLElement.prototype.hasPointerCapture = () => false;
  dom.window.HTMLElement.prototype.releasePointerCapture = () => {};
  dom.window.HTMLElement.prototype.scrollIntoView = () => {};
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  dom.window.__TAURI_INTERNALS__ = {
    invoke: (cmd, args) => {
      const handler = ipcHandlers.get(cmd);
      if (handler) return handler(args);
      unmocked.push(cmd);
      return Promise.reject(new Error(`unmocked Tauri command: ${cmd}`));
    },
    transformCallback: () => Math.random(),
  };

  ({ act, cleanup, fireEvent, render, screen } = await import(
    "@testing-library/react"
  ));
  ({ createElement } = await import("react"));
  ({ QueryClient, QueryClientProvider } = await import(
    "@tanstack/react-query"
  ));
  ({ ThemeProvider } = await import("@/shared/theme/ThemeProvider"));
  ({ AgentDialog } = await import("./AgentDialog.tsx"));
  ({ editPersonaDialogState } = await import("./personaDialogState.ts"));
  ({ fromRawAcpRuntimeCatalogEntry } = await import(
    "../../../shared/api/tauri.ts"
  ));
});

afterEach(() => {
  cleanup?.();
  for (const client of clients.splice(0)) {
    client.cancelQueries();
    client.clear();
  }
  ipcHandlers.clear();
  unmocked.length = 0;
});

after(() => dom.window.close());

test("Edit: the effort picker is offered beside the model control and preselects the stored definition effort", async () => {
  installIpc();
  const submitted = await openEditDialog();

  const picker = dom.window.document.getElementById("persona-effort");
  assert.ok(
    picker,
    "edit mode must offer the picker for an effort-capable harness",
  );
  assert.ok(
    picker.closest('[data-testid="model-effort-fields"]'),
    "the picker is the model control's companion, not a stray field",
  );
  assert.ok(
    dom.window.document.getElementById("persona-model"),
    "the model control renders in the same pairing",
  );
  assert.ok(
    picker.textContent?.includes("High"),
    `the stored effort preselects its option; got "${picker.textContent}"`,
  );

  // An untouched save must round-trip the stored default, not clear it.
  const input = await save(submitted);
  assert.equal(input.id, "p-1");
  assert.equal(input.effortLevel, "high");
});

test("Edit: choosing the sentinel submits an explicit null so the default is cleared", async () => {
  installIpc();
  const submitted = await openEditDialog();

  await pickFromDropdown(
    "persona-effort",
    (label) => label === "Adapter default",
  );

  const input = await save(submitted);
  assert.equal(
    input.effortLevel,
    null,
    "null (not absent) is the tri-state clear the Rust update applies",
  );
});

test("Edit: a new pick replaces the stored default", async () => {
  installIpc();
  const submitted = await openEditDialog();

  await pickFromDropdown("persona-effort", (label) => label === "Low");

  const input = await save(submitted);
  assert.equal(input.effortLevel, "low");
});

test("Edit: switching to a harness with no effort vocabulary hides the picker and clears the stale default", async () => {
  installIpc();
  const submitted = await openEditDialog();
  assert.ok(dom.window.document.getElementById("persona-effort"));

  await pickFromDropdown("persona-runtime", (label) =>
    label.toLowerCase().startsWith("cursor"),
  );
  await settle(50);

  assert.equal(
    dom.window.document.getElementById("persona-effort"),
    null,
    "no catalog effortThoughtLevel → the companion hides",
  );

  const input = await save(submitted);
  assert.equal(input.runtime, "cursor", "the harness switch is submitted");
  assert.equal(
    input.effortLevel,
    null,
    "the vocabulary belongs to the harness: a stale pick is cleared, not carried",
  );
});

test("Edit: a harness with no effort vocabulary never offers the picker and leaves the stored default untouched", async () => {
  installIpc();
  const submitted = await openEditDialog(
    storedPersona({ runtime: "cursor", effortLevel: null }),
  );
  assert.equal(dom.window.document.getElementById("persona-effort"), null);

  const input = await save(submitted);
  assert.equal(
    "effortLevel" in input && input.effortLevel !== undefined,
    false,
    "absent = don't touch: an unrelated edit on such a harness never rewrites the column",
  );
});
