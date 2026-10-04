import assert from "node:assert/strict";
import test from "node:test";

// The Agents page card menu is the one place a user can flip "Start on
// launch" (the Runtime tab only reports it). These tests bind the production
// seam end to end: the real `useManagedAgentActions.handleToggleStartOnAppLaunch`
// behind the real checkbox item, down to the Tauri command and its payload.

const LOCAL_PUBKEY = "ab".repeat(32);
const PROVIDER_PUBKEY = "cd".repeat(32);
const calls = [];

function rawAgent(overrides = {}) {
  return {
    pubkey: LOCAL_PUBKEY,
    name: "Scout",
    persona_id: null,
    relay_url: "wss://relay.example",
    acp_command: "buzz-acp",
    agent_command: "claude",
    agent_args: [],
    mcp_command: "",
    turn_timeout_seconds: 0,
    idle_timeout_seconds: 0,
    max_turn_duration_seconds: 0,
    parallelism: 1,
    system_prompt: "",
    model: null,
    env_vars: {},
    status: "stopped",
    pid: null,
    created_at: 1,
    updated_at: 1,
    last_started_at: null,
    last_stopped_at: null,
    last_exit_code: null,
    last_error: null,
    log_path: null,
    start_on_app_launch: true,
    backend: { type: "local" },
    backend_agent_id: null,
    respond_to_allowlist: [],
    ...overrides,
  };
}

const localAgent = rawAgent();
const providerAgent = rawAgent({
  pubkey: PROVIDER_PUBKEY,
  name: "Remote",
  start_on_app_launch: false,
  backend: { type: "provider", id: "blox", config: {} },
});

const tauriMock = {
  invoke(command, args) {
    calls.push({ command, args });
    switch (command) {
      case "list_managed_agents":
        return Promise.resolve([localAgent, providerAgent]);
      case "set_managed_agent_start_on_app_launch": {
        const agent = [localAgent, providerAgent].find(
          (entry) => entry.pubkey === args.pubkey,
        );
        return Promise.resolve({
          ...agent,
          start_on_app_launch: args.startOnAppLaunch,
        });
      }
      default:
        return Promise.reject(new Error(`unmocked Tauri command: ${command}`));
    }
  },
  transformCallback() {
    return Math.random();
  },
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_INTERNALS__ = tauriMock;
// Radix's popper positions the open menu through floating-ui, which observes
// the anchor with ResizeObserver; jsdom has none.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const { fireEvent } = await import("@testing-library/react");
const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { QueryClient, QueryClientProvider } = await import(
  "@tanstack/react-query"
);
const { CommunitiesProvider } = await import(
  "../../communities/useCommunities.tsx"
);
const { DropdownMenu, DropdownMenuContent } = await import(
  "@/shared/ui/dropdown-menu"
);
const { AgentStartOnLaunchMenuItem } = await import(
  "./AgentStartOnLaunchMenuItem.tsx"
);
const { useManagedAgentActions } = await import("./useManagedAgentActions.ts");

function Harness({ pubkey, latest }) {
  const agents = useManagedAgentActions();
  Object.assign(latest, agents);
  const agent = agents.managedAgents.find((entry) => entry.pubkey === pubkey);
  return React.createElement(
    DropdownMenu,
    { modal: false, open: true },
    React.createElement(
      DropdownMenuContent,
      null,
      React.createElement(AgentStartOnLaunchMenuItem, {
        agent,
        isPending: agents.isStartOnLaunchPending,
        // Same one-line adapter `AgentsView` uses.
        onToggleStartOnLaunch: (target, next) => {
          void agents.handleToggleStartOnAppLaunch(target.pubkey, next);
        },
      }),
    ),
  );
}

async function settle(predicate) {
  for (let i = 0; i < 40 && !predicate(); i += 1) {
    await act(() => new Promise((resolve) => setTimeout(resolve, 5)));
  }
}

async function mount(pubkey) {
  calls.length = 0;
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  const latest = {};
  await act(async () => {
    root.render(
      React.createElement(
        QueryClientProvider,
        { client: queryClient },
        React.createElement(
          CommunitiesProvider,
          null,
          React.createElement(Harness, { pubkey, latest }),
        ),
      ),
    );
  });
  const item = () =>
    document.querySelector(`[data-testid='agent-start-on-launch-${pubkey}']`);
  await settle(() => item() !== null);
  assert.ok(item(), "the menu item rendered once the agent list loaded");
  return {
    item,
    latest,
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

test("a local agent gets a checked menuitemcheckbox that owns its own label", async () => {
  const { item, unmount } = await mount(LOCAL_PUBKEY);
  assert.equal(item().getAttribute("role"), "menuitemcheckbox");
  assert.equal(item().getAttribute("aria-checked"), "true");
  assert.equal(item().hasAttribute("aria-label"), false);
  assert.equal(item().textContent.trim(), "Start on launch");
  assert.equal(item().getAttribute("data-disabled"), null);
  await unmount();
});

test("clicking the item sends the inverted flag through the production handler", async () => {
  const { item, latest, unmount } = await mount(LOCAL_PUBKEY);

  await act(async () => {
    fireEvent.click(item());
  });
  const writes = calls.filter(
    (call) => call.command === "set_managed_agent_start_on_app_launch",
  );
  assert.equal(writes.length, 1, "exactly one persist per click");
  assert.deepEqual(writes[0].args, {
    pubkey: LOCAL_PUBKEY,
    startOnAppLaunch: false,
  });
  await settle(() => latest.actionNoticeMessage !== null);
  assert.equal(
    latest.actionNoticeMessage,
    "Scout will stay manual-start only.",
  );
  await unmount();
});

test("a provider-backed agent shows the item disabled as managed by its provider", async () => {
  const { item, unmount } = await mount(PROVIDER_PUBKEY);
  assert.equal(item().getAttribute("role"), "menuitemcheckbox");
  assert.equal(item().getAttribute("aria-checked"), "false");
  assert.equal(item().getAttribute("aria-disabled"), "true");
  assert.ok(item().textContent.includes("Managed by provider"));

  await act(async () => {
    fireEvent.click(item());
  });
  assert.equal(
    calls.some(
      (call) => call.command === "set_managed_agent_start_on_app_launch",
    ),
    false,
    "a disabled item never writes",
  );
  await unmount();
});

test("without an instance there is no item to render", async () => {
  const container = document.createElement("div");
  const root = createRoot(container);
  await act(async () => {
    root.render(
      React.createElement(
        DropdownMenu,
        { modal: false, open: true },
        React.createElement(
          DropdownMenuContent,
          null,
          React.createElement(AgentStartOnLaunchMenuItem, {
            agent: undefined,
            isPending: false,
            onToggleStartOnLaunch: () => assert.fail("nothing to toggle"),
          }),
        ),
      ),
    );
  });
  assert.equal(document.querySelector("[role='menuitemcheckbox']"), null);
  await act(async () => root.unmount());
});
