import assert from "node:assert/strict";
import test from "node:test";

// Pins the star action's IPC contract: starring sends the agent's pubkey to
// `set_default_managed_agent`, un-starring the current default sends `null`,
// and the notice reflects the new state. Rust owns single selection, so the
// hook must never try to unstar "the other" agent itself.

const STARRED = "ab".repeat(32);
const OTHER = "cd".repeat(32);
const RELAY = "wss://relay.example";
/** @type {{ command: string, payload: unknown }[]} */
const calls = [];

function rawAgent(pubkey, name, isDefaultAi) {
  return {
    pubkey,
    name,
    persona_id: null,
    relay_url: RELAY,
    acp_command: "acp",
    agent_command: "agent",
    agent_args: [],
    mcp_command: "mcp",
    turn_timeout_seconds: 60,
    idle_timeout_seconds: 60,
    max_turn_duration_seconds: 60,
    parallelism: 1,
    system_prompt: null,
    model: null,
    status: "stopped",
    pid: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    last_started_at: null,
    last_stopped_at: null,
    last_exit_code: null,
    last_error: null,
    log_path: "/tmp/log",
    start_on_app_launch: false,
    is_default_ai: isDefaultAi,
    backend: { type: "local" },
    backend_agent_id: null,
  };
}

let agents = [rawAgent(STARRED, "Scout", true), rawAgent(OTHER, "Pal", false)];

const tauriMock = {
  invoke(command, payload) {
    calls.push({ command, payload });
    if (command === "list_managed_agents") return Promise.resolve(agents);
    if (command === "set_default_managed_agent") {
      // Mirror the native command: at most one star, `null` clears all.
      agents = agents.map((agent) => ({
        ...agent,
        is_default_ai:
          payload.pubkey !== null && agent.pubkey === payload.pubkey,
      }));
      return Promise.resolve(agents);
    }
    return Promise.reject(new Error(`unmocked Tauri command: ${command}`));
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
const { useManagedAgentActions } = await import("./useManagedAgentActions.ts");
const { CommunitiesProvider } = await import(
  "../../communities/useCommunities.tsx"
);
const { saveActiveCommunityId, saveCommunities } = await import(
  "../../communities/communityStorage.ts"
);

saveCommunities([{ id: "a", name: "A", relayUrl: RELAY }]);
saveActiveCommunityId("a");

async function renderActions() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const root = createRoot(document.createElement("div"));
  const latest = {};
  function Harness() {
    Object.assign(latest, useManagedAgentActions());
    return null;
  }
  await act(async () => {
    root.render(
      React.createElement(
        QueryClientProvider,
        { client: queryClient },
        React.createElement(
          CommunitiesProvider,
          null,
          React.createElement(Harness),
        ),
      ),
    );
  });
  for (let i = 0; i < 20 && (latest.managedAgents?.length ?? 0) < 2; i += 1) {
    await act(() => new Promise((resolve) => setTimeout(resolve, 5)));
  }
  assert.equal(latest.managedAgents?.length, 2, "agent list loaded");
  return { latest, unmount: () => act(async () => root.unmount()) };
}

function defaultAiCalls() {
  return calls.filter((call) => call.command === "set_default_managed_agent");
}

test("starring an agent sends its pubkey and reports the new default", async () => {
  calls.length = 0;
  const { latest, unmount } = await renderActions();

  await act(async () => {
    await latest.handleToggleDefaultAi(OTHER, true);
  });

  assert.deepEqual(
    defaultAiCalls().map((call) => call.payload),
    [{ pubkey: OTHER }],
    "exactly one IPC call carrying the starred pubkey",
  );
  assert.equal(latest.actionNoticeMessage, "Pal is now your default AI.");
  assert.equal(latest.actionErrorMessage, null);
  await unmount();
});

test("un-starring the current default clears the selection with null", async () => {
  calls.length = 0;
  agents = [rawAgent(STARRED, "Scout", true), rawAgent(OTHER, "Pal", false)];
  const { latest, unmount } = await renderActions();

  const scout = latest.managedAgents.find((agent) => agent.pubkey === STARRED);
  assert.equal(scout.isDefaultAi, true, "fixture starts starred");

  // The row toggles with `!agent.isDefaultAi`, so the starred agent asks
  // for `false`, which must become a `null` selection, not a re-star.
  await act(async () => {
    await latest.handleToggleDefaultAi(scout.pubkey, !scout.isDefaultAi);
  });

  assert.deepEqual(
    defaultAiCalls().map((call) => call.payload),
    [{ pubkey: null }],
  );
  assert.equal(
    latest.actionNoticeMessage,
    "Scout is no longer your default AI.",
  );
  assert.equal(latest.actionErrorMessage, null);
  await unmount();
});

test("a rejected star surfaces the backend error and no notice", async () => {
  calls.length = 0;
  const { latest, unmount } = await renderActions();
  const original = tauriMock.invoke;
  tauriMock.invoke = (command, payload) => {
    if (command === "set_default_managed_agent") {
      calls.push({ command, payload });
      return Promise.reject(
        new Error("The default AI must be a deployed agent with its own key."),
      );
    }
    return original(command, payload);
  };

  await act(async () => {
    await latest.handleToggleDefaultAi(OTHER, true);
  });

  assert.equal(
    latest.actionErrorMessage,
    "The default AI must be a deployed agent with its own key.",
  );
  assert.equal(latest.actionNoticeMessage, null);
  tauriMock.invoke = original;
  await unmount();
});
