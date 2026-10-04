import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

// Settings › Models › Providers against the Tauri IPC: one user action is one
// `set_global_agent_config` carrying the whole record, an API-key save never
// touches `model`, the single "Default for new agents" radio group writes the
// harness switch the way Agent defaults does, and the tabs/radios carry
// their accessible roles with one label owner each.

// ── Browser API stubs ────────────────────────────────────────────────────────

// Radix Tabs' roving focus calls the bare `requestAnimationFrame` global.
if (!globalThis.requestAnimationFrame) {
  const raf = (cb) => {
    setTimeout(cb, 0);
    return 0;
  };
  globalThis.requestAnimationFrame = raf;
  globalThis.window.requestAnimationFrame = raf;
  globalThis.cancelAnimationFrame = () => {};
  globalThis.window.cancelAnimationFrame = () => {};
}
if (!globalThis.window.matchMedia) {
  globalThis.window.matchMedia = () => ({
    matches: false,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  });
}
if (!globalThis.ResizeObserver) {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}

// ── Tauri IPC mock ───────────────────────────────────────────────────────────

let calls = [];
let globalConfig;

function rawRuntime(overrides) {
  return {
    id: "custom",
    label: "Custom",
    avatar_url: "",
    availability: "available",
    command: "custom",
    binary_path: "/bin/custom",
    default_args: [],
    mcp_command: null,
    model_env_var: null,
    provider_env_var: null,
    thinking_env_var: null,
    install_hint: "",
    install_instructions_url: "",
    can_auto_install: false,
    requires_external_cli: false,
    underlying_cli_path: null,
    node_required: false,
    auth_status: { status: "not_applicable" },
    source: "builtin",
    ...overrides,
  };
}

const rawCatalog = [
  rawRuntime({
    id: "goose",
    label: "Goose",
    model_env_var: "GOOSE_MODEL",
    provider_env_var: "GOOSE_PROVIDER",
  }),
  rawRuntime({
    id: "claude",
    label: "Claude Code",
    requires_external_cli: true,
    auth_status: { status: "logged_in" },
    subscription_provider: "anthropic",
  }),
  rawRuntime({
    id: "codex",
    label: "Codex",
    availability: "not_installed",
    command: null,
    binary_path: null,
    requires_external_cli: true,
    auth_status: { status: "unknown" },
    subscription_provider: "openai",
  }),
  rawRuntime({
    id: "buzz-agent",
    label: "Buzz Agent",
    model_env_var: "BUZZ_AGENT_MODEL",
    provider_env_var: "BUZZ_AGENT_PROVIDER",
  }),
];

const tauriMock = {
  invoke(command, args) {
    calls.push({ command, args });
    switch (command) {
      case "discover_acp_providers":
        return Promise.resolve(rawCatalog);
      case "get_global_agent_config":
        return Promise.resolve(globalConfig);
      case "set_global_agent_config":
        globalConfig = args.config;
        return Promise.resolve({
          config: args.config,
          restarted_count: 2,
          failed_restart_count: 0,
        });
      case "get_baked_build_env_keys":
        return Promise.resolve([]);
      case "discover_acp_auth_methods":
        return Promise.resolve({ methods: [] });
      default:
        return new Promise(() => {}); // pending — never an unmocked error
    }
  },
  transformCallback() {
    return Math.random();
  },
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_INTERNALS__ = tauriMock;

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { QueryClient, QueryClientProvider } = await import(
  "@tanstack/react-query"
);
const { ProvidersSettingsTab } = await import("./ProvidersSettingsTab.tsx");
const { ModelsSettingsPanel } = await import("./ModelsSettingsPanel.tsx");

// ── Harness ──────────────────────────────────────────────────────────────────

const setCalls = () =>
  calls.filter((call) => call.command === "set_global_agent_config");

async function settle(ms = 20) {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, ms));
  });
}

async function waitFor(container, selector, attempts = 40) {
  for (let i = 0; i < attempts; i++) {
    const found = container.querySelector(selector);
    if (found) return found;
    await settle(10);
  }
  throw new Error(`timed out waiting for ${selector}`);
}

async function mount(Component) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(
      React.createElement(
        QueryClientProvider,
        { client: queryClient },
        React.createElement(Component),
      ),
    );
  });
  return {
    container,
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

function typeInto(input, value) {
  const setter = Object.getOwnPropertyDescriptor(
    globalThis.window.HTMLInputElement.prototype,
    "value",
  ).set;
  setter.call(input, value);
  input.dispatchEvent(new globalThis.window.Event("input", { bubbles: true }));
}

let mounted = null;

afterEach(async () => {
  if (mounted) {
    await mounted.unmount();
    mounted = null;
  }
  calls = [];
});

function resetConfig() {
  globalConfig = {
    env_vars: { OPENAI_COMPAT_API_KEY: "sk-openai" },
    provider: "anthropic",
    model: "claude-sonnet",
    preferred_runtime: "buzz-agent",
    effort_level: "high",
  };
}

// ── Tests ────────────────────────────────────────────────────────────────────

test("saving an API key is one set_global_agent_config that leaves model untouched", async () => {
  resetConfig();
  mounted = await mount(ProvidersSettingsTab);
  const { container } = mounted;
  const keyRow = await waitFor(
    container,
    '[data-testid="settings-models-anthropic-api-key"]',
  );
  const input = keyRow.querySelector(
    'input[data-testid="persona-provider-api-key"]',
  );
  assert.ok(input, "the shared API key field renders");
  const save = container.querySelector(
    '[data-testid="settings-models-anthropic-save-key"]',
  );
  assert.equal(save.disabled, true, "Save is idle until a key is typed");

  await act(async () => typeInto(input, " sk-ant-test "));
  assert.equal(save.disabled, false);
  await act(async () => save.click());
  await settle(30);

  const saves = setCalls();
  assert.equal(saves.length, 1, "exactly one persist for one Save");
  const sent = saves[0].args.config;
  assert.equal(sent.env_vars.ANTHROPIC_API_KEY, "sk-ant-test");
  assert.equal(
    sent.env_vars.OPENAI_COMPAT_API_KEY,
    "sk-openai",
    "other keys kept",
  );
  assert.equal(sent.model, "claude-sonnet", "model is never written here");
  assert.equal(sent.provider, "anthropic");
  assert.equal(sent.preferred_runtime, "buzz-agent");
  assert.equal(sent.effort_level, "high");

  const notice = await waitFor(
    container,
    '[data-testid="settings-models-save-notice"]',
  );
  assert.equal(notice.textContent, "Saved. Restarted 2 agents.");
  assert.equal(
    input.value,
    "",
    "the draft clears; the saved key is never echoed",
  );
  assert.ok(
    container.querySelector(
      '[data-testid="settings-models-anthropic-remove-key"]',
    ),
    "a set key offers Remove",
  );
});

test("removing a key is one persist with an empty value", async () => {
  resetConfig();
  mounted = await mount(ProvidersSettingsTab);
  const { container } = mounted;
  const remove = await waitFor(
    container,
    '[data-testid="settings-models-openai-remove-key"]',
  );
  await act(async () => remove.click());
  await settle(30);
  const saves = setCalls();
  assert.equal(saves.length, 1);
  assert.equal(saves[0].args.config.env_vars.OPENAI_COMPAT_API_KEY, "");
  assert.equal(saves[0].args.config.model, "claude-sonnet");
});

test("the OpenAI key row names OpenAI-compatible as a reader instead of a second field", async () => {
  resetConfig();
  mounted = await mount(ProvidersSettingsTab);
  const { container } = mounted;
  const openai = await waitFor(
    container,
    '[data-testid="settings-models-openai-api-key"]',
  );
  assert.match(openai.textContent, /Also used by OpenAI-compatible agents/);
  assert.equal(
    container.querySelector(
      '[data-testid="settings-models-openai-compat-api-key"]',
    ),
    null,
  );
  assert.equal(
    container.querySelectorAll('input[data-testid="persona-provider-api-key"]')
      .length,
    3,
    "anthropic, openai, openrouter",
  );
  // Subscription rows reuse HarnessRow for the login-billed harness only.
  assert.ok(
    container.querySelector(
      '[data-testid="settings-models-anthropic-subscription"] [data-testid="doctor-runtime-claude"]',
    ),
  );
  assert.ok(
    container.querySelector(
      '[data-testid="settings-models-openai-subscription"] [data-testid="doctor-runtime-codex"]',
    ),
  );
  assert.equal(
    container.querySelector(
      '[data-testid="settings-models-openrouter-subscription"]',
    ),
    null,
  );
});

test("the default radio group is one group with one checked option and one label owner each", async () => {
  resetConfig();
  mounted = await mount(ProvidersSettingsTab);
  const { container } = mounted;
  await waitFor(
    container,
    '[data-testid="settings-models-default-anthropic:api-key"]',
  );
  const groups = container.querySelectorAll('[role="radiogroup"]');
  assert.equal(groups.length, 1, "exactly one radio group across providers");
  const group = groups[0];
  const title = document.getElementById(group.getAttribute("aria-labelledby"));
  assert.equal(title?.textContent, "Default for new agents");

  const radios = [...group.querySelectorAll('input[type="radio"]')];
  assert.ok(radios.length >= 6);
  assert.equal(radios.filter((radio) => radio.checked).length, 1);
  assert.equal(
    radios.find((radio) => radio.checked).value,
    "anthropic:api-key",
  );
  for (const radio of radios) {
    assert.equal(
      container.querySelectorAll(`label[for="${radio.id}"]`).length,
      1,
      `${radio.value} has one label`,
    );
    assert.equal(radio.hasAttribute("aria-label"), false);
  }
  // Codex is not installed: its option is offered but locked.
  const codex = radios.find((radio) => radio.value === "openai:subscription");
  assert.equal(codex.disabled, true);
  assert.match(
    container.querySelector(
      '[data-testid="settings-models-default-openai:subscription"]',
    ).textContent,
    /not installed/,
  );
});

test("choosing a subscription default persists the harness switch once, clearing model and effort", async () => {
  resetConfig();
  mounted = await mount(ProvidersSettingsTab);
  const { container } = mounted;
  await waitFor(
    container,
    '[data-testid="settings-models-default-anthropic:subscription"]',
  );
  const radio = container.querySelector(
    'input[type="radio"][value="anthropic:subscription"]',
  );
  assert.equal(radio.disabled, false);
  await act(async () => radio.click());
  await settle(30);

  const saves = setCalls();
  assert.equal(saves.length, 1);
  const sent = saves[0].args.config;
  assert.equal(sent.preferred_runtime, "claude");
  assert.equal(sent.model, null);
  assert.equal(sent.effort_level, null);
  assert.equal(sent.provider, null, "a login harness takes no provider");
  assert.equal(
    sent.env_vars.OPENAI_COMPAT_API_KEY,
    "sk-openai",
    "keys survive",
  );

  await settle(30);
  const checked = [...container.querySelectorAll('input[type="radio"]')].filter(
    (candidate) => candidate.checked,
  );
  assert.deepEqual(
    checked.map((candidate) => candidate.value),
    ["anthropic:subscription"],
  );
  // Re-choosing the current default writes nothing.
  await act(async () => radio.click());
  await settle(20);
  assert.equal(setCalls().length, 1);
});

test("Models renders an accessible tablist and switches to the Task models empty state", async () => {
  resetConfig();
  mounted = await mount(ModelsSettingsPanel);
  const { container } = mounted;
  const tablist = container.querySelector('[role="tablist"]');
  assert.ok(tablist, "tabs expose role=tablist");
  assert.equal(tablist.getAttribute("aria-label"), "Models settings");
  const tabs = [...tablist.querySelectorAll('[role="tab"]')];
  assert.deepEqual(
    tabs.map((tab) => tab.textContent),
    ["Providers", "Task models"],
  );
  assert.equal(tabs[0].getAttribute("aria-selected"), "true");
  await waitFor(container, '[data-testid="settings-models-providers"]');
  assert.equal(
    container.querySelector('[data-testid="settings-models-tasks-empty"]'),
    null,
  );

  await act(async () => {
    tabs[1].dispatchEvent(
      new globalThis.window.MouseEvent("mousedown", {
        bubbles: true,
        button: 0,
      }),
    );
  });
  await settle(20);
  assert.equal(tabs[1].getAttribute("aria-selected"), "true");
  const empty = container.querySelector(
    '[data-testid="settings-models-tasks-empty"]',
  );
  assert.ok(empty, "Task models shows its empty state");
  assert.match(empty.textContent, /No app tasks yet/);
  assert.equal(
    container.querySelector('[data-testid="settings-models-providers"]'),
    null,
    "only the active tab's panel is mounted",
  );
});
