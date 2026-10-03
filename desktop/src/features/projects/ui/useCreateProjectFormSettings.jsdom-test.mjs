import assert from "node:assert/strict";
import test from "node:test";

// The project create form's `addDefaultAi` derivation: seeded from the stored
// preference, flippable, and forced to `false` while no agent is starred.

const PUBKEY = "ab".repeat(32);

let agents = [];
const tauriMock = {
  invoke(command) {
    if (command === "list_managed_agents") return Promise.resolve(agents);
    // Personas, teams, runtimes, templates: empty lists are enough here.
    return Promise.resolve([]);
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
const { setDefaultAiAutoJoin } = await import(
  "../../agents/lib/defaultAiPreferences.ts"
);
const { useCreateProjectFormSettings } = await import(
  "./useCreateProjectFormSettings.ts"
);

function rawAgent(overrides = {}) {
  return {
    pubkey: PUBKEY,
    name: "Scout",
    agent_args: [],
    env_vars: {},
    status: "running",
    is_default_ai: true,
    backend: { type: "local" },
    respond_to_allowlist: [],
    ...overrides,
  };
}

function Harness({ handle }) {
  handle.current = useCreateProjectFormSettings(true);
  return null;
}

async function mount({ expectDefaultAi }) {
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
  for (
    let i = 0;
    i < 20 && Boolean(handle.current.defaultAi) !== expectDefaultAi;
    i++
  ) {
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 5));
    });
  }
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 10));
  });
  assert.equal(Boolean(handle.current.defaultAi), expectDefaultAi);
  return {
    settings: () => handle.current,
    unmount: () => act(async () => root.unmount()),
  };
}

test("with a starred agent addDefaultAi follows the preference and the switch", async () => {
  agents = [rawAgent()];
  setDefaultAiAutoJoin(true);
  const harness = await mount({ expectDefaultAi: true });

  assert.equal(harness.settings().addDefaultAi, true);
  await act(async () => harness.settings().setAddDefaultAi(false));
  assert.equal(harness.settings().addDefaultAi, false);
  await harness.unmount();
});

test("without a starred agent addDefaultAi is forced false even with the preference on", async () => {
  agents = [rawAgent({ is_default_ai: false })];
  setDefaultAiAutoJoin(true);
  const harness = await mount({ expectDefaultAi: false });

  assert.equal(harness.settings().defaultAi, null);
  assert.equal(harness.settings().addDefaultAi, false);
  // Flipping the hidden switch on cannot produce a true payload.
  await act(async () => harness.settings().setAddDefaultAi(true));
  assert.equal(harness.settings().addDefaultAi, false);
  await harness.unmount();
});
