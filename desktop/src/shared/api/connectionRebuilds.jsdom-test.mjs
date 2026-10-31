/**
 * Launch and reconnect load the read state and the home (activity) feed
 * once, not once per relay connection-state step: the hooks key on the
 * account and on "connected", and the connection state reports a change
 * only when it differs. Mobile had this defect (its session state had no
 * equality and its providers watched the whole session); this pins desktop.
 *
 * The connection steps go through the production emitter, the read state
 * through the production hook and manager, the feed through the production
 * query hook and its real Tauri command name.
 *
 * Falsifiability: keying useReadState's manager effect on the connection
 * state, or the home feed query on it, makes each step fetch again.
 */

import assert from "node:assert/strict";
import { afterEach, mock, test } from "node:test";

const commands = [];
const tauriMock = {
  invoke(command) {
    commands.push(command);
    if (command === "get_feed") {
      return Promise.resolve({
        feed: {
          mentions: [],
          needs_action: [],
          activity: [],
          agent_activity: [],
        },
        meta: { since: 0, total: 0, generated_at: 0 },
      });
    }
    return Promise.reject(new Error(`unmocked Tauri command: ${command}`));
  },
  transformCallback: () => Math.random(),
  unregisterCallback() {},
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_INTERNALS__ = tauriMock;

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { QueryClient, QueryClientProvider } = await import(
  "@tanstack/react-query"
);
const { relayClient } = await import("@/shared/api/relayClient");
const { RelayConnectionStateEmitter } = await import(
  "@/shared/api/relayConnectionStateEmitter"
);
const { useReadState } = await import(
  "@/features/channels/readState/useReadState"
);
const { useHomeFeedQuery } = await import("@/features/home/hooks");
const { KIND_READ_STATE } = await import("@/shared/constants/kinds");

const PUBKEY = "a".repeat(64);

afterEach(() => {
  mock.restoreAll();
  commands.length = 0;
});

function Harness() {
  useReadState(PUBKEY, relayClient);
  useHomeFeedQuery();
  return null;
}

async function settle() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 10));
  });
}

test("launch and a reconnect fetch the read state and the home feed once", async () => {
  const emitter = new RelayConnectionStateEmitter("idle");
  const counts = { readStateFetches: 0, readStateSubscribes: 0 };
  mock.method(relayClient, "getConnectionState", () => emitter.get());
  mock.method(relayClient, "subscribeToConnectionState", (listener) =>
    emitter.subscribe(listener),
  );
  mock.method(relayClient, "fetchEvents", async (filter) => {
    if (filter.kinds?.includes(KIND_READ_STATE)) counts.readStateFetches++;
    return [];
  });
  mock.method(relayClient, "subscribeInteractive", async (filter) => {
    if (filter.kinds?.includes(KIND_READ_STATE)) counts.readStateSubscribes++;
    return () => {};
  });
  mock.method(relayClient, "publishEvent", async () => ({}));

  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const container = document.createElement("div");
  const root = createRoot(container);
  await act(async () => {
    root.render(
      React.createElement(
        QueryClientProvider,
        { client: queryClient },
        React.createElement(Harness),
      ),
    );
  });
  await settle();

  const step = async (state) => {
    await act(async () => emitter.set(state));
    await settle();
  };
  const feedFetches = () =>
    commands.filter((command) => command === "get_feed").length;

  // Launch.
  await step("connecting");
  await step("connected");
  assert.equal(counts.readStateFetches, 1);
  assert.equal(counts.readStateSubscribes, 1);
  assert.equal(feedFetches(), 1);

  // A drop, a failed attempt, the next attempt, connected again (the same
  // state set twice is not a change).
  await step("reconnecting");
  await step("reconnecting");
  await step("connecting");
  await step("reconnecting");
  await step("connecting");
  await step("connected");
  // The relay client replays live subscriptions on reconnect; the read
  // state and the feed are not refetched for status steps.
  assert.equal(counts.readStateFetches, 1);
  assert.equal(counts.readStateSubscribes, 1);
  assert.equal(feedFetches(), 1);

  await act(async () => root.unmount());
  queryClient.clear();
});
