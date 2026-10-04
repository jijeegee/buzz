import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

// Agents page › Channel routing against the Tauri IPC: the picker renders
// only under a selected Host, a radio alone never saves a half-configured
// mode, picking an agent is exactly one set_channel_routing carrying mode and
// agent, and the status line + Restart now follow the Rust transition plan.

const HONEY = "aa".repeat(32);
const FIZZ = "bb".repeat(32);
const HOST = "cc".repeat(32);

let calls = [];
let routing;

const tauriMock = {
  invoke(command, args) {
    calls.push({ command, args });
    switch (command) {
      case "get_channel_routing":
        return Promise.resolve(routing);
      case "set_channel_routing":
        routing = {
          ...routing,
          mode: args.mode,
          routingAgent: args.agentPubkey ?? routing.routingAgent,
        };
        return Promise.resolve(routing);
      case "list_personas":
        return Promise.resolve([
          {
            id: "builtin:host",
            display_name: "Host",
            avatar_url: null,
            system_prompt: "You are Host.",
            effort_level: "low",
            is_builtin: true,
            is_active: true,
            created_at: "",
            updated_at: "",
          },
        ]);
      case "discover_acp_providers":
        return Promise.resolve([
          {
            id: "buzz-agent",
            label: "Buzz Agent",
            availability: "available",
            command: "buzz-agent",
            default_args: [],
            mcp_command: null,
            source: "builtin",
          },
        ]);
      case "get_global_agent_config":
        return Promise.resolve({ preferred_runtime: null });
      case "create_managed_agent":
        return Promise.resolve({
          agent: {
            pubkey: HOST,
            name: args.input.name,
            persona_id: args.input.personaId,
            backend: { type: "local" },
          },
        });
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
const { ChannelRoutingCard } = await import("./ChannelRoutingCard.tsx");
const { reportChannelBotTyping, resetAgentWorkingSignal } = await import(
  "../../agentWorkingSignal.ts"
);

const AGENTS = [
  { pubkey: HONEY, name: "Honey" },
  { pubkey: FIZZ, name: "Fizz" },
];

function baseRouting(overrides = {}) {
  return {
    mode: "host",
    routingAgent: HONEY,
    applied: { state: "hosting", pubkey: HONEY },
    routerActive: false,
    agents: [
      {
        pubkey: HONEY,
        running: true,
        local: true,
        runningRole: "dispatcher",
        desiredRole: "dispatcher",
        stale: false,
        hold: false,
      },
    ],
    ...overrides,
  };
}

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

let mounted = null;
let restarts = [];

async function mount(agents = AGENTS) {
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
        React.createElement(ChannelRoutingCard, {
          agents,
          onRestartAgent: (pubkey) => restarts.push(pubkey),
          restartingAgentPubkey: null,
        }),
      ),
    );
  });
  await waitFor(container, '[data-testid="agents-channel-routing-status"]');
  mounted = {
    container,
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
  return container;
}

afterEach(async () => {
  if (mounted) {
    await mounted.unmount();
    mounted = null;
  }
  calls = [];
  restarts = [];
});

const setCalls = () =>
  calls.filter((call) => call.command === "set_channel_routing");
const radio = (container, mode) =>
  container.querySelector(`input[type="radio"][value="${mode}"]`);
const picker = (container) =>
  container.querySelector('[data-testid="agents-channel-routing-picker"]');
const statusText = (container) =>
  container
    .querySelector('[data-testid="agents-channel-routing-status"]')
    .textContent.trim();

async function click(element) {
  await act(async () => {
    element.click();
  });
  await settle();
}

async function choose(select, value) {
  await act(async () => {
    select.value = value;
    select.dispatchEvent(
      new globalThis.window.Event("change", { bubbles: true }),
    );
  });
  await settle();
}

test("one label owner each: the group is named once by the title, every control by its own label", async () => {
  routing = baseRouting();
  const container = await mount();

  const group = container.querySelector(
    '[data-testid="agents-channel-routing-modes"]',
  );
  const titleId = group.getAttribute("aria-labelledby");
  assert.equal(document.getElementById(titleId).textContent, "Channel routing");
  assert.equal(
    container
      .querySelector('[data-testid="agents-channel-routing"]')
      .hasAttribute("aria-labelledby"),
    false,
    "the section must not repeat the group's label",
  );

  const radios = [...container.querySelectorAll('input[type="radio"]')];
  assert.deepEqual(
    radios.map((input) => input.value),
    ["off", "host", "lead", "desktop-router"],
  );
  for (const input of radios) {
    assert.equal(input.labels.length, 1, `${input.value} has one label`);
  }
  assert.deepEqual(
    radios.map((input) => input.disabled),
    [false, false, true, true],
    "Lead and Smart routing are not selectable yet",
  );
  assert.match(radio(container, "lead").labels[0].textContent, /Coming soon/);

  const select = container.querySelector(
    '[data-testid="agents-channel-routing-agent-select"]',
  );
  assert.equal(select.labels.length, 1);
  assert.equal(select.labels[0].textContent, "Host agent");
  assert.equal(select.value, HONEY);
  const autoJoin = container.querySelector(
    '[data-testid="agents-channel-routing-auto-join"]',
  );
  assert.equal(autoJoin.getAttribute("role"), "checkbox");
  assert.equal(autoJoin.labels.length, 1);
  assert.equal(autoJoin.labels[0].textContent, "Join new channels I create");

  assert.equal(statusText(container), "On — Honey is hosting.");
});

test("the picker renders only under a selected Host", async () => {
  routing = baseRouting({ mode: "off", applied: { state: "off" } });
  const container = await mount();
  assert.equal(picker(container), null);
  assert.equal(statusText(container), "Off — only @mentioned agents answer.");
});

test("Host with a remembered agent saves once; Off saves once with no agent", async () => {
  routing = baseRouting({ mode: "off", applied: { state: "off" } });
  const container = await mount();

  await click(radio(container, "host"));
  assert.deepEqual(
    setCalls().map((call) => call.args),
    [{ mode: "host", agentPubkey: HONEY }],
  );
  assert.ok(picker(container), "the picker unfolds under Host");

  await click(radio(container, "off"));
  assert.deepEqual(
    setCalls().map((call) => call.args),
    [
      { mode: "host", agentPubkey: HONEY },
      { mode: "off", agentPubkey: null },
    ],
  );
  assert.equal(picker(container), null);
});

test("without a routing agent the radio only opens the picker; the pick is the one save", async () => {
  routing = baseRouting({
    mode: "off",
    routingAgent: null,
    applied: { state: "off" },
    agents: [],
  });
  const container = await mount();

  await click(radio(container, "host"));
  assert.equal(setCalls().length, 0, "a radio click alone never saves");
  assert.equal(radio(container, "host").checked, true);
  assert.ok(
    container.querySelector(
      '[data-testid="agents-channel-routing-draft-hint"]',
    ),
  );
  const select = container.querySelector(
    '[data-testid="agents-channel-routing-agent-select"]',
  );
  assert.equal(select.value, "", "nothing chosen yet");

  await choose(select, FIZZ);
  assert.deepEqual(
    setCalls().map((call) => call.args),
    [{ mode: "host", agentPubkey: FIZZ }],
  );
  assert.equal(
    container.querySelector(
      '[data-testid="agents-channel-routing-draft-hint"]',
    ),
    null,
  );
});

test("with no agents the picker says so instead of rendering an empty select", async () => {
  routing = baseRouting({ routingAgent: null, agents: [] });
  const container = await mount([]);
  assert.ok(
    container.querySelector('[data-testid="agents-channel-routing-no-agents"]'),
  );
  assert.equal(
    container.querySelector(
      '[data-testid="agents-channel-routing-agent-select"]',
    ),
    null,
  );
});

test("with no agents, Host offers to create one and saves it as the host", async () => {
  routing = baseRouting({
    mode: "off",
    routingAgent: null,
    applied: { state: "off" },
    agents: [],
  });
  const container = await mount([]);

  await click(radio(container, "host"));
  const create = await waitFor(
    container,
    '[data-testid="agents-channel-routing-create-host"]',
  );
  assert.equal(create.textContent, "Create a Host agent");

  await click(create);
  for (let i = 0; i < 40 && setCalls().length === 0; i++) await settle(10);

  const created = calls.filter(
    (call) => call.command === "create_managed_agent",
  );
  assert.equal(created.length, 1);
  assert.equal(created[0].args.input.personaId, "builtin:host");
  assert.equal(created[0].args.input.name, "Host");
  assert.deepEqual(
    setCalls().map((call) => call.args),
    [{ mode: "host", agentPubkey: HOST }],
  );
});

test("while switching hosts, only the losing agent can restart now", async () => {
  routing = baseRouting({
    routingAgent: FIZZ,
    applied: { state: "switching" },
    agents: [
      {
        pubkey: HONEY,
        running: true,
        local: true,
        runningRole: "dispatcher",
        desiredRole: "none",
        stale: true,
        hold: false,
      },
      {
        pubkey: FIZZ,
        running: true,
        local: true,
        runningRole: "none",
        desiredRole: "dispatcher",
        stale: true,
        hold: true,
      },
    ],
  });
  const container = await mount();
  assert.match(statusText(container), /^Switching — Honey stops hosting/);

  const rows = [
    ...container.querySelectorAll(
      '[data-testid="agents-channel-routing-restart"]',
    ),
  ];
  assert.equal(rows.length, 2);
  const [honeyButton, fizzButton] = rows.map((row) =>
    row.querySelector("button"),
  );
  assert.equal(honeyButton.textContent, "Restart Honey now");
  assert.equal(honeyButton.disabled, false);
  assert.equal(fizzButton.disabled, true);
  assert.match(rows[1].textContent, /Waiting for Honey to stop hosting/);

  await click(honeyButton);
  assert.deepEqual(restarts, [HONEY]);
  assert.equal(setCalls().length, 0, "restarting never rewrites the mode");
});

test("a mid-turn agent's Restart now disables live and a stale click does nothing", async () => {
  routing = baseRouting({
    mode: "off",
    applied: { state: "switching" },
    agents: [
      {
        pubkey: HONEY,
        running: true,
        local: true,
        runningRole: "dispatcher",
        desiredRole: "none",
        stale: true,
        hold: false,
      },
    ],
  });
  const container = await mount();
  const button = () =>
    container.querySelector(
      '[data-testid="agents-channel-routing-restart"] button',
    );
  assert.equal(button().disabled, false);

  // Honey starts a turn after render: the card re-renders from the signal.
  await act(async () => reportChannelBotTyping("chan-1", [HONEY]));
  assert.equal(button().disabled, true);
  assert.match(
    container.querySelector('[data-testid="agents-channel-routing-restart"]')
      .textContent,
    /middle of a turn/,
  );

  // Turn ends: the button comes back and a click restarts once.
  await act(async () => reportChannelBotTyping("chan-1", []));
  assert.equal(button().disabled, false);
  await click(button());
  assert.deepEqual(restarts, [HONEY]);
  resetAgentWorkingSignal();
});
