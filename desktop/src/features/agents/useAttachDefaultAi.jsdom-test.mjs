import assert from "node:assert/strict";
import test from "node:test";

// `useAttachDefaultAi` is the single path that puts the starred default AI
// into a channel the user just created. These tests pin its contract against
// the Tauri IPC: membership write first, start only for a non-running agent,
// a no-op without a starred agent, and a warning toast (never a rejection)
// when the membership write fails.

const PUBKEY = "ab".repeat(32);
const OTHER = "cd".repeat(32);
const CHANNEL = "11111111-1111-4111-8111-111111111111";

let calls = [];
let agents = [];
let membershipError = null;

function rawAgent(overrides = {}) {
  return {
    pubkey: PUBKEY,
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
    start_on_app_launch: false,
    is_default_ai: true,
    backend: { type: "local" },
    backend_agent_id: null,
    respond_to_allowlist: [],
    ...overrides,
  };
}

const tauriMock = {
  invoke(command, args) {
    calls.push({ command, args });
    switch (command) {
      case "list_managed_agents":
        return Promise.resolve(agents);
      case "add_channel_members":
        return Promise.resolve(
          membershipError
            ? {
                added: [],
                errors: [{ pubkey: args.pubkeys[0], error: membershipError }],
              }
            : { added: args.pubkeys, errors: [] },
        );
      case "start_managed_agent": {
        const agent = agents.find((entry) => entry.pubkey === args.pubkey);
        return Promise.resolve({ ...agent, status: "running" });
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

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { QueryClient, QueryClientProvider } = await import(
  "@tanstack/react-query"
);
const { toast } = await import("sonner");
const { useAttachDefaultAi } = await import("./useAttachDefaultAi.ts");
const { managedAgentsQueryKey } = await import("./hooks.ts");

const warnings = [];
toast.warning = (message, options) => {
  warnings.push({ message, options });
  return "toast-id";
};

function Harness({ handle }) {
  const result = useAttachDefaultAi();
  handle.current = result;
  return null;
}

async function mount() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const root = createRoot(document.createElement("div"));
  const handle = { current: null };
  await act(async () => {
    root.render(
      React.createElement(
        QueryClientProvider,
        { client: queryClient },
        React.createElement(Harness, { handle }),
      ),
    );
  });
  // Let the managed-agents query settle so `hasDefaultAi` reflects the mock.
  const expectDefaultAi = agents.some((agent) => agent.is_default_ai);
  for (
    let i = 0;
    i < 20 && handle.current?.hasDefaultAi !== expectDefaultAi;
    i++
  ) {
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 5));
    });
  }
  // Give a no-default-AI list time to load so the no-op is a settled state.
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 10));
  });
  return {
    attach: (channelId) => act(() => handle.current.attachDefaultAi(channelId)),
    /** The callback as rendered right now — captured to test staleness. */
    attachFn: () => handle.current.attachDefaultAi,
    hasDefaultAi: () => handle.current.hasDefaultAi,
    queryClient,
    unmount: () => act(async () => root.unmount()),
  };
}

// Only the two commands the attach contract is about: other modules fire
// unrelated IPCs (relay origin, media proxy port) on their own schedule.
const ATTACH_COMMANDS = new Set(["add_channel_members", "start_managed_agent"]);
function commandNames() {
  return calls
    .map((call) => call.command)
    .filter((command) => ATTACH_COMMANDS.has(command));
}

test.beforeEach(() => {
  calls = [];
  warnings.length = 0;
  membershipError = null;
});

test("a stopped default AI is added as a bot and then started", async () => {
  agents = [rawAgent({ status: "stopped" })];
  const harness = await mount();
  assert.equal(harness.hasDefaultAi(), true);

  await harness.attach(CHANNEL);

  assert.deepEqual(commandNames(), [
    "add_channel_members",
    "start_managed_agent",
  ]);
  const membership = calls.find((c) => c.command === "add_channel_members");
  assert.deepEqual(membership.args, {
    channelId: CHANNEL,
    pubkeys: [PUBKEY],
    role: "bot",
  });
  const start = calls.find((c) => c.command === "start_managed_agent");
  assert.equal(start.args.pubkey, PUBKEY);
  assert.equal(warnings.length, 0);
  // The attach refreshes the managed list like `applyAgents` does.
  assert.ok(
    calls.filter((c) => c.command === "list_managed_agents").length >= 2,
  );
  await harness.unmount();
});

test("a running default AI only gets the membership write", async () => {
  agents = [rawAgent({ status: "running" })];
  const harness = await mount();

  await harness.attach(CHANNEL);

  assert.deepEqual(commandNames(), ["add_channel_members"]);
  assert.equal(warnings.length, 0);
  await harness.unmount();
});

test("without a starred agent the attach is a no-op", async () => {
  agents = [
    rawAgent({ is_default_ai: false }),
    rawAgent({ pubkey: OTHER, is_default_ai: false }),
  ];
  const harness = await mount();
  assert.equal(harness.hasDefaultAi(), false);

  await harness.attach(CHANNEL);

  assert.deepEqual(commandNames(), []);
  assert.equal(warnings.length, 0);
  await harness.unmount();
});

test("a callback captured while an agent was starred attaches nothing once the cache no longer stars it", async () => {
  agents = [rawAgent({ status: "stopped" })];
  const harness = await mount();
  assert.equal(harness.hasDefaultAi(), true);
  const staleAttach = harness.attachFn();

  // The star moves away / the record is deleted: the cache is the truth.
  await act(async () => {
    harness.queryClient.setQueryData(managedAgentsQueryKey, []);
  });
  await act(() => staleAttach(CHANNEL));

  assert.deepEqual(commandNames(), []);
  assert.equal(warnings.length, 0);
  await harness.unmount();
});

test("a failed membership write warns instead of rejecting and skips the start", async () => {
  agents = [rawAgent({ status: "stopped" })];
  membershipError = "relay rejected the membership update";
  const harness = await mount();

  await harness.attach(CHANNEL);

  assert.deepEqual(commandNames(), ["add_channel_members"]);
  assert.equal(warnings.length, 1);
  assert.match(warnings[0].message, /Scout could not be added/);
  assert.match(
    warnings[0].options.description,
    /relay rejected the membership update/,
  );
  await harness.unmount();
});
