import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

// Settings › Models › Task models › Message routing against the Tauri IPC:
// route and model only (no effort control), each change is exactly one
// `set_task_model`, routes that aren't set up (no key, CLI signed out)
// cannot be picked, subscription routes list their own models, and the row
// reports readiness from Rust.

let calls = [];
let status;

const PROVIDERS = [
  {
    id: "anthropic",
    label: "Anthropic API key",
    kind: "api-key",
    ready: true,
    unavailableReason: null,
    defaultModel: "claude-haiku-4-5",
    models: [],
    sendWaitMs: 1_200,
  },
  {
    id: "openai",
    label: "OpenAI API key",
    kind: "api-key",
    ready: false,
    unavailableReason: "Needs an OpenAI API key",
    defaultModel: "gpt-4.1-nano",
    models: [],
    sendWaitMs: 1_200,
  },
  {
    id: "codex",
    label: "Codex (ChatGPT subscription)",
    kind: "subscription",
    ready: true,
    unavailableReason: null,
    defaultModel: "gpt-6-luna",
    models: ["gpt-6-luna", "gpt-6-sol"],
    sendWaitMs: 8_000,
  },
  {
    id: "claude-code",
    label: "Claude Code (Claude subscription, slower)",
    kind: "subscription",
    ready: false,
    unavailableReason: "Sign in to Claude Code",
    defaultModel: "haiku",
    models: ["haiku", "sonnet"],
    sendWaitMs: 8_000,
  },
];

function statusFor({ provider = null, model = null } = {}) {
  const effective = provider ?? "anthropic";
  const route = PROVIDERS.find((entry) => entry.id === effective);
  return {
    taskId: "message-routing",
    provider,
    model,
    effectiveProvider: effective,
    effectiveModel: model ?? route.defaultModel,
    modelLabel:
      model ?? (effective === "anthropic" ? "Claude Haiku 4.5" : "GPT-6-Luna"),
    ready: true,
    notReadyReason: null,
    sendWaitMs: route.sendWaitMs,
    providers: PROVIDERS,
  };
}

const tauriMock = {
  invoke(command, args) {
    calls.push({ command, args });
    switch (command) {
      case "get_task_models":
        return Promise.resolve([status]);
      case "set_task_model":
        status = statusFor({ provider: args.provider, model: args.model });
        return Promise.resolve([status]);
      case "get_global_agent_config":
        return Promise.resolve({
          env_vars: { ANTHROPIC_API_KEY: "sk-test" },
          provider: null,
          model: null,
          preferred_runtime: null,
          effort_level: null,
        });
      case "discover_agent_models":
        return Promise.resolve({
          agentName: "buzz-agent",
          agentVersion: "0",
          models: [
            { id: "claude-haiku-4-5", name: null, description: null },
            { id: "claude-sonnet-5-5", name: null, description: null },
          ],
          agentDefaultModel: null,
          selectedModel: null,
          supportsSwitching: true,
        });
      default:
        return new Promise(() => {});
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
const { TaskModelsSettingsTab } = await import("./TaskModelsSettingsTab.tsx");

async function settle(ms = 20) {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, ms));
  });
}

async function waitFor(predicate, attempts = 60) {
  for (let i = 0; i < attempts; i++) {
    const found = predicate();
    if (found) return found;
    await settle(10);
  }
  throw new Error("timed out");
}

let mounted = null;

async function mount() {
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
        React.createElement(TaskModelsSettingsTab, {
          onOpenProviders: () => {},
        }),
      ),
    );
  });
  mounted = { root, container };
  await waitFor(() =>
    container.querySelector(
      '[data-testid="settings-models-task-message-routing"]',
    ),
  );
  return container;
}

afterEach(async () => {
  if (mounted) {
    await act(async () => mounted.root.unmount());
    mounted.container.remove();
    mounted = null;
  }
  calls = [];
});

const select = (container, which) =>
  container.querySelector(
    `[data-testid="settings-models-task-message-routing-${which}"]`,
  );
const setCalls = () =>
  calls.filter((call) => call.command === "set_task_model");

async function choose(element, value) {
  await act(async () => {
    element.value = value;
    element.dispatchEvent(new window.Event("change", { bubbles: true }));
  });
  await settle();
}

test("provider and model only: two labelled selects and no effort control", async () => {
  status = statusFor();
  const container = await mount();
  const provider = select(container, "provider");
  const model = select(container, "model");
  assert.equal(provider.labels[0].textContent, "Provider");
  assert.equal(model.labels[0].textContent, "Model");
  // Rule 14's wrapper renders only the model controls: no effort companion.
  const fields = container.querySelector('[data-testid="model-effort-fields"]');
  assert.equal(fields.children.length, 1);
  assert.doesNotMatch(fields.textContent, /effort/i);
  assert.equal(container.querySelectorAll("select").length, 2);
  assert.match(
    select(container, "status").textContent,
    /Ready — Claude Haiku 4\.5 via Anthropic API key/,
  );
  assert.match(container.textContent, /including drafts while you type/);
});

test("every route is listed; ones that aren't set up say why and cannot be picked", async () => {
  status = statusFor();
  const container = await mount();
  const options = [...select(container, "provider").options].map((option) => [
    option.value,
    option.disabled,
    option.textContent,
  ]);
  assert.deepEqual(options, [
    ["", false, "Automatic"],
    ["anthropic", false, "Anthropic API key"],
    ["openai", true, "OpenAI API key — Needs an OpenAI API key"],
    ["codex", false, "Codex (ChatGPT subscription)"],
    [
      "claude-code",
      true,
      "Claude Code (Claude subscription, slower) — Sign in to Claude Code",
    ],
  ]);
});

test("a subscription route offers its own models without key discovery", async () => {
  status = statusFor({ provider: "codex" });
  const container = await mount();
  const model = select(container, "model");
  assert.deepEqual(
    [...model.options].map((option) => option.value),
    ["", "gpt-6-sol"],
  );
  assert.match(model.options[0].textContent, /gpt-6-luna \(fastest\)/);
  assert.match(
    select(container, "status").textContent,
    /Codex \(ChatGPT subscription\)\. About 5 s per pick/,
  );
  assert.equal(
    calls.filter((call) => call.command === "discover_agent_models").length,
    0,
  );
  await choose(model, "gpt-6-sol");
  assert.deepEqual(setCalls().at(-1).args, {
    taskId: "message-routing",
    provider: "codex",
    model: "gpt-6-sol",
  });
});

test("each change is exactly one set_task_model; a provider change resets the model", async () => {
  status = statusFor();
  const container = await mount();
  const model = await waitFor(() => {
    const element = select(container, "model");
    return [...element.options].some((o) => o.value === "claude-sonnet-5-5")
      ? element
      : null;
  });
  // The provider default is the "Default" row, not a duplicate entry.
  assert.deepEqual(
    [...model.options].map((option) => option.value),
    ["", "claude-sonnet-5-5"],
  );
  assert.match(model.options[0].textContent, /claude-haiku-4-5 \(cheapest\)/);

  await choose(model, "claude-sonnet-5-5");
  assert.deepEqual(
    setCalls().map((call) => call.args),
    [{ taskId: "message-routing", provider: null, model: "claude-sonnet-5-5" }],
  );

  await choose(select(container, "provider"), "anthropic");
  assert.deepEqual(setCalls().at(-1).args, {
    taskId: "message-routing",
    provider: "anthropic",
    model: null,
  });
  assert.equal(setCalls().length, 2);

  await choose(select(container, "model"), "");
  assert.deepEqual(setCalls().at(-1).args, {
    taskId: "message-routing",
    provider: "anthropic",
    model: null,
  });
  assert.equal(setCalls().length, 3);
  assert.equal(
    calls.filter((call) => call.command === "set_global_agent_config").length,
    0,
    "a task model change never writes the global agent config",
  );
});
