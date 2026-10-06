/**
 * Create-dialog effort acceptance pins (production seam).
 *
 * The create-time effort travels a long way: `EffortPickerField` (fed by the
 * runtime catalog's `effortThoughtLevel.fallbackValues`, since no session has
 * run yet, paired with the model control by `ModelEffortFields`) →
 * `AgentDefinitionDialog.handleSubmit` → the `CreatePersonaInput` →
 * `AgentDialog`'s create router → `usePersonaActions.handleSubmit` →
 * `createPersona` → the `create_persona` IPC payload. A hand-written
 * miniature cannot catch a regression at any one of those hops, so these tests
 * mount the real `RequestedAgentCreateDialogs` (the app-level Create entry
 * point, which owns the real `usePersonaActions`), open it through the real
 * `requestOpenCreateAgent` event, drive the REAL effort dropdown and the REAL
 * "Create agent" button, and assert what reaches the mocked IPC boundary:
 *   - a picked effort lands as `input.effortLevel` on `create_persona` — the
 *     definition-level default every linked instance launches with — and is
 *     NOT copied onto `create_managed_agent`, whose column would freeze it and
 *     mask every later definition edit;
 *   - an untouched picker sends no `effortLevel` at all (absent, not "").
 *
 * The harness is the global-default Claude Code runtime in "Use defaults"
 * mode, which also pins that effort is offered outside Customize: it rides
 * with whatever model choice is shown, inherited defaults included.
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
let CommunitiesProvider;
let RequestedAgentCreateDialogs;
let requestOpenCreateAgent;

// Records every Tauri command invocation; unmocked commands reject (and are
// recorded) so a new IPC dependency surfaces loudly instead of silently.
const ipcCalls = [];
const unmocked = [];
const ipcHandlers = new Map();

const OWNER_PK = "a".repeat(64);
const AGENT_PK = "d".repeat(64);

// The saved global default harness; a test can flip it mid-dialog to model the
// in-dialog "Edit defaults" save, which the create dialog observes through the
// same global-config query refetch.
let globalPreferredRuntime = "claude";

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
// catalog; Goose does not (its effort is its own env knob).
const CLAUDE_THOUGHT_LEVEL = {
  config_option_id: "effort",
  fallback_values: ["low", "medium", "high"],
};

function rawPersona(input) {
  return {
    id: "p-created",
    display_name: input.displayName,
    description: input.description ?? null,
    avatar_url: input.avatarUrl ?? null,
    system_prompt: input.systemPrompt ?? "",
    acp_command: input.acpCommand ?? "buzz-acp",
    runtime: input.runtime ?? null,
    model: input.model ?? null,
    provider: input.provider ?? null,
    effort_level: input.effortLevel ?? null,
    name_pool: [],
    is_builtin: false,
    is_active: true,
    shared: false,
    source_team: null,
    env_vars: input.envVars ?? {},
    respond_to: null,
    respond_to_allowlist: [],
    parallelism: null,
    session_policy: input.behavior?.sessionPolicy ?? "channel",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
  };
}

function rawAgent(input) {
  return {
    pubkey: AGENT_PK,
    name: input.name,
    persona_id: input.personaId ?? null,
    runtime: "claude",
    relay_url: "wss://relay.example",
    acp_command: input.acpCommand ?? "buzz-acp",
    agent_command: input.agentCommand ?? "claude-cmd",
    agent_command_override: null,
    agent_args: [],
    mcp_command: "",
    turn_timeout_seconds: 300,
    idle_timeout_seconds: null,
    max_turn_duration_seconds: null,
    parallelism: 1,
    system_prompt: null,
    avatar_url: null,
    model: null,
    provider: null,
    persona_out_of_date: false,
    persona_orphaned: false,
    needs_restart: false,
    env_vars: {},
    status: "running",
    pid: 1234,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    last_started_at: null,
    last_stopped_at: null,
    last_exit_code: null,
    last_error: null,
    last_error_code: null,
    log_path: "/tmp/agent.log",
    start_on_app_launch: true,
    auto_restart_on_config_change: true,
    backend: { type: "local" },
    backend_agent_id: null,
    respond_to: "owner-only",
    respond_to_allowlist: [],
  };
}

function installIpc() {
  const set = (cmd, handler) => ipcHandlers.set(cmd, handler);
  set("get_identity", () =>
    Promise.resolve({ pubkey: OWNER_PK, npub: `npub1${"a".repeat(58)}` }),
  );
  set("list_personas", () => Promise.resolve([]));
  set("list_managed_agents", () => Promise.resolve([]));
  set("discover_acp_providers", () =>
    Promise.resolve([
      rawRuntime("claude", { effort_thought_level: CLAUDE_THOUGHT_LEVEL }),
      rawRuntime("goose"),
      // A harness with no effort vocabulary and no provider/model requirement,
      // so re-seeding onto it leaves Create submittable in "Use defaults".
      rawRuntime("cursor"),
    ]),
  );
  // Claude Code is the global default harness, so Create opens on it in
  // "Use defaults" mode — the picker must be offered there too.
  set("get_global_agent_config", () =>
    Promise.resolve({
      env_vars: {},
      provider: null,
      model: null,
      preferred_runtime: globalPreferredRuntime,
    }),
  );
  set("get_baked_build_env", () => Promise.resolve([]));
  set("get_baked_build_env_keys", () => Promise.resolve([]));
  set("get_runtime_file_config", () => Promise.resolve(null));
  set("agent_access_owner_only", () => Promise.resolve(false));
  set("discover_backend_providers", () => Promise.resolve([]));
  // Fire-and-forget post-create runtime reconcile; not under test here.
  set("reconcile_managed_agent_runtimes", () => Promise.resolve([]));
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
  set("create_persona", (args) => {
    ipcCalls.push({ cmd: "create_persona", args });
    return Promise.resolve(rawPersona(args.input));
  });
  // The persistence boundary under test.
  set("create_managed_agent", (args) => {
    ipcCalls.push({ cmd: "create_managed_agent", args });
    return Promise.resolve({
      agent: rawAgent(args.input),
      private_key_nsec: `nsec1${"b".repeat(58)}`,
      profile_sync_error: null,
      spawn_error: null,
    });
  });
}

function renderCreateEntryPoint() {
  const client = new QueryClient({
    defaultOptions: {
      mutations: { gcTime: 0 },
      queries: { gcTime: 0, retry: false },
    },
  });
  clients.push(client);
  return render(
    createElement(
      CommunitiesProvider,
      null,
      createElement(
        ThemeProvider,
        { defaultTheme: "buzz" },
        createElement(
          QueryClientProvider,
          { client },
          createElement(RequestedAgentCreateDialogs),
        ),
      ),
    ),
  );
}

async function settle(ms = 20) {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, ms));
  });
}

async function openCreateDialog() {
  await act(async () => {
    renderCreateEntryPoint();
  });
  await act(async () => {
    requestOpenCreateAgent();
  });
  // Let the runtime catalog + defaults queries resolve so the default runtime
  // seeds and the effort picker can mount.
  await settle(50);
  const nameInput = dom.window.document.getElementById("persona-display-name");
  assert.ok(nameInput, "Create dialog must open with the display-name field");
  return nameInput;
}

async function selectEffort(label) {
  const trigger = dom.window.document.getElementById("persona-effort");
  assert.ok(
    trigger,
    "effort picker must render in Create for a local Run-on on a catalog-fallback runtime before any session exists",
  );
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
  assert.ok(item, `effort option "${label}" must be offered`);
  await act(async () => {
    fireEvent.click(item);
  });
}

async function submitCreate() {
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "Create agent" }));
  });
  // create_persona then create_managed_agent are sequential awaits.
  for (let i = 0; i < 50; i += 1) {
    if (ipcCalls.some((c) => c.cmd === "create_managed_agent")) break;
    await settle(20);
  }
  const creates = ipcCalls.filter((c) => c.cmd === "create_managed_agent");
  assert.equal(
    creates.length,
    1,
    `Create must dispatch exactly one create_managed_agent (unmocked: ${unmocked.join(", ") || "none"})`,
  );
  const personas = ipcCalls.filter((c) => c.cmd === "create_persona");
  assert.equal(
    personas.length,
    1,
    "Create must dispatch exactly one create_persona",
  );
  return { persona: personas[0].args.input, instance: creates[0].args.input };
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
  ({ CommunitiesProvider } = await import(
    "@/features/communities/useCommunities"
  ));
  ({ RequestedAgentCreateDialogs } = await import(
    "./RequestedAgentCreateDialogs.tsx"
  ));
  ({ requestOpenCreateAgent } = await import("../openCreateAgentEvent.ts"));
});

afterEach(() => {
  globalPreferredRuntime = "claude";
  cleanup?.();
  for (const client of clients.splice(0)) {
    client.cancelQueries();
    client.clear();
  }
  ipcHandlers.clear();
  ipcCalls.length = 0;
  unmocked.length = 0;
});

after(() => dom.window.close());

test("Create: a picked effort reaches create_persona as the definition default, never the instance column", async () => {
  installIpc();
  const nameInput = await openCreateDialog();
  await act(async () => {
    fireEvent.change(nameInput, { target: { value: "Effortful" } });
  });

  // "High" comes from the catalog fallback — no session has ever run, so there
  // is no discovered option list to pick from.
  await selectEffort("High");

  const { persona, instance } = await submitCreate();
  assert.equal(
    persona.effortLevel,
    "high",
    "the Create pick must travel in the create_persona payload",
  );
  assert.equal(
    instance.agentCommand,
    "claude-cmd",
    "the picked harness is Claude",
  );
  assert.deepEqual(instance.backend, { type: "local" });
  assert.equal(
    "effortLevel" in instance,
    false,
    "the instance column stays unset so the definition default applies at spawn and later edits propagate",
  );
});

test("Create: untouched defaults omit effortLevel and persist thread context", async () => {
  installIpc();
  const nameInput = await openCreateDialog();
  await act(async () => {
    fireEvent.change(nameInput, { target: { value: "Default effort" } });
  });
  assert.ok(
    dom.window.document.getElementById("persona-effort"),
    "the picker is offered (so its absence from the payload is a choice, not a missing control)",
  );

  const { persona, instance } = await submitCreate();
  // The mock sees the raw payload object; Tauri's JSON serialization drops
  // an undefined value, which is the "absent" the Rust request reads as
  // "adapter default".
  assert.equal(
    persona.effortLevel,
    undefined,
    "no pick must mean the key is absent — the definition default stays adapter default",
  );
  assert.equal("effortLevel" in instance, false);
  assert.equal(persona.behavior.sessionPolicy, "main_and_threads");
});

test("Create: choosing Entire channel persists the explicit policy", async () => {
  installIpc();
  const nameInput = await openCreateDialog();
  await act(async () => {
    fireEvent.change(nameInput, { target: { value: "Shared context" } });
    fireEvent.click(screen.getByRole("button", { name: /Advanced/ }));
  });
  const trigger = dom.window.document.getElementById("persona-session-policy");
  assert.ok(trigger, "Conversation context must be available in Advanced");
  assert.equal(trigger.textContent.trim(), "Main timeline and each thread");
  await act(async () => {
    fireEvent.pointerDown(
      trigger,
      new dom.window.MouseEvent("pointerdown", { bubbles: true, button: 0 }),
    );
    fireEvent.click(trigger);
  });
  await act(async () => {
    fireEvent.click(
      screen.getByRole("menuitemradio", { name: "Entire channel" }),
    );
  });
  const { persona } = await submitCreate();
  assert.equal(persona.behavior.sessionPolicy, "channel");
});

test("Create: a pick made before saved defaults re-seed a no-effort harness is dropped", async () => {
  // Create opens auto-seeded on Claude; the user picks High, then saves new
  // global defaults in the in-dialog defaults editor that make Cursor the
  // default harness. The runtime re-seeds to Cursor (no catalog
  // effortThoughtLevel, so the picker hides) and the stale "high" must not
  // ride along in create_managed_agent — Cursor never advertised that
  // vocabulary. Modeled through the production seam the in-dialog save uses:
  // the global-config query refetches with the new preferred runtime.
  installIpc();
  const nameInput = await openCreateDialog();
  await act(async () => {
    fireEvent.change(nameInput, { target: { value: "Re-seeded" } });
  });
  await selectEffort("High");

  globalPreferredRuntime = "cursor";
  const client = clients[clients.length - 1];
  await act(async () => {
    await client.invalidateQueries();
  });
  await settle(50);

  assert.equal(
    dom.window.document.getElementById("persona-effort"),
    null,
    "the picker must hide once the seeded harness publishes no effort vocabulary",
  );

  const { persona, instance } = await submitCreate();
  assert.equal(
    instance.agentCommand,
    "cursor-cmd",
    "the re-seeded harness is Cursor",
  );
  assert.equal(
    persona.effortLevel,
    undefined,
    "the pre-re-seed pick must not travel for a harness the picker was hidden for",
  );
});
