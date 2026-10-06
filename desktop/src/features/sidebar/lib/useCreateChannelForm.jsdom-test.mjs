import assert from "node:assert/strict";
import test from "node:test";

// The create-channel form's `addDefaultAi` payload: seeded from the stored
// preference, flippable per form, and always `false` when no agent is starred.

const PUBKEY = "ab".repeat(32);

let agents = [];
let routingMode = "host";
const tauriMock = {
  invoke(command) {
    switch (command) {
      case "get_channel_routing":
        return Promise.resolve({
          mode: routingMode,
          routingAgent: null,
          applied: { state: "off" },
          routerActive: false,
          agents: [],
        });
      case "list_channel_templates":
        return Promise.resolve([]);
      case "list_managed_agents":
        return Promise.resolve(agents);
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
const { setDefaultAiAutoJoin } = await import(
  "../../agents/lib/defaultAiPreferences.ts"
);
const { useCreateChannelForm } = await import("./useCreateChannelForm.ts");

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

function Harness({ handle, active, onCreate }) {
  handle.current = useCreateChannelForm({
    channelKind: "stream",
    active,
    isCreating: false,
    onCreate,
    autoFocusName: false,
  });
  return null;
}

async function mount({ expectDefaultAi }) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const root = createRoot(document.createElement("div"));
  const handle = { current: null };
  const created = [];
  const onCreate = async (input) => {
    created.push(input);
  };
  const render = (active) =>
    act(async () => {
      root.render(
        React.createElement(
          QueryClientProvider,
          { client: queryClient },
          React.createElement(Harness, { handle, active, onCreate }),
        ),
      );
    });
  await render(true);
  for (
    let i = 0;
    i < 20 && Boolean(handle.current.defaultAi) !== expectDefaultAi;
    i++
  ) {
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 5));
    });
  }
  // Let a no-default-AI list finish loading before asserting on it.
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 10));
  });
  assert.equal(Boolean(handle.current.defaultAi), expectDefaultAi);
  for (
    let i = 0;
    i < 20 &&
    handle.current.defaultAiJoinsNewChannels !==
      (routingMode === "host" || routingMode === "lead");
    i++
  ) {
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 5));
    });
  }
  const submit = async () => {
    await act(async () => handle.current.setName("release-notes"));
    await act(async () => {
      handle.current.handleSubmit({ preventDefault() {} });
    });
    await act(async () => {});
    return created.at(-1);
  };
  return {
    form: () => handle.current,
    reopen: async () => {
      await render(false);
      await render(true);
    },
    submit,
    unmount: () => act(async () => root.unmount()),
  };
}

test("with a starred agent the payload follows the stored preference and the switch", async () => {
  agents = [rawAgent()];
  setDefaultAiAutoJoin(true);
  const harness = await mount({ expectDefaultAi: true });

  assert.equal(harness.form().addDefaultAi, true);
  let payload = await harness.submit();
  assert.equal(payload.addDefaultAi, true);
  assert.equal(payload.name, "release-notes");

  await act(async () => harness.form().setAddDefaultAi(false));
  payload = await harness.submit();
  assert.equal(payload.addDefaultAi, false);

  // Reopening the form re-seeds from the preference.
  setDefaultAiAutoJoin(false);
  await harness.reopen();
  assert.equal(harness.form().addDefaultAi, false);
  payload = await harness.submit();
  assert.equal(payload.addDefaultAi, false);

  setDefaultAiAutoJoin(true);
  await harness.unmount();
});

test("without a starred agent the payload is false even when the preference is on", async () => {
  agents = [rawAgent({ is_default_ai: false })];
  setDefaultAiAutoJoin(true);
  const harness = await mount({ expectDefaultAi: false });

  assert.equal(harness.form().defaultAi, null);
  const payload = await harness.submit();
  assert.equal(payload.addDefaultAi, false);
  await harness.unmount();
});

test("under Off or Smart routing the row is hidden and the payload never joins", async () => {
  for (const mode of ["off", "desktop-router"]) {
    routingMode = mode;
    agents = [rawAgent()];
    setDefaultAiAutoJoin(true);
    const harness = await mount({ expectDefaultAi: true });

    assert.equal(harness.form().defaultAiJoinsNewChannels, false, mode);
    const payload = await harness.submit();
    assert.equal(payload.addDefaultAi, false, mode);
    await harness.unmount();
  }
  routingMode = "host";
});
