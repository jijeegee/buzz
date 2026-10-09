import assert from "node:assert/strict";
import test, { mock } from "node:test";

// `useObserverWatching` is what every live session panel calls. These tests
// bind its production path: the app-wide registry, the relay connection
// state, and the Tauri-built control frame published through relayClient.

const AGENT = "ab".repeat(32);
const built = [];
const tauriMock = {
  invoke(command, args) {
    if (command === "build_observer_control_event") {
      built.push(args);
      return Promise.resolve(
        JSON.stringify({ id: "e", kind: 24200, tags: [], content: "x" }),
      );
    }
    return Promise.reject(new Error(`unmocked Tauri command: ${command}`));
  },
  transformCallback() {
    return Math.random();
  },
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_INTERNALS__ = tauriMock;

let visibility = "visible";
Object.defineProperty(document, "visibilityState", {
  configurable: true,
  get: () => visibility,
});

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { relayClient } = await import("@/shared/api/relayClient");
const {
  useObserverWatching,
  getObserverWatchingRegistry,
  _resetObserverWatchingRegistryForTests,
} = await import("./useObserverWatching.ts");
const { isLiveSessionView } = await import("./ui/agentSessionPanelLayout.ts");

const published = [];
const connectionListeners = new Set();
let connection = "connected";
mock.method(relayClient, "getConnectionState", () => connection);
mock.method(relayClient, "subscribeToConnectionState", (listener) => {
  connectionListeners.add(listener);
  listener(connection);
  return () => connectionListeners.delete(listener);
});
mock.method(relayClient, "preconnect", async () => {});
mock.method(relayClient, "publishEvent", async (event) => {
  published.push(event);
});

function Panel({ agentPubkey, channelId, live }) {
  useObserverWatching(agentPubkey, channelId, live);
  return null;
}

async function flush() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

async function render(root, panels) {
  await act(async () => {
    root.render(
      React.createElement(
        React.Fragment,
        null,
        ...panels.map((props, index) =>
          React.createElement(Panel, { key: index, ...props }),
        ),
      ),
    );
  });
  await flush();
}

function reset() {
  built.length = 0;
  published.length = 0;
  connection = "connected";
  visibility = "visible";
  _resetObserverWatchingRegistryForTests();
}

test("a visible live panel sends one watching frame on open", async () => {
  reset();
  const root = createRoot(document.createElement("div"));
  await render(root, [{ agentPubkey: AGENT, channelId: "chan-1", live: true }]);
  assert.equal(published.length, 1);
  assert.deepEqual(built, [
    {
      agentPubkey: AGENT,
      payload: { type: "watching", channelId: "chan-1" },
    },
  ]);
  await act(async () => root.unmount());
  assert.deepEqual(getObserverWatchingRegistry().heldAgents(), []);
});

test("two live panels for one agent send once", async () => {
  reset();
  const root = createRoot(document.createElement("div"));
  await render(root, [
    { agentPubkey: AGENT, channelId: "chan-1", live: true },
    { agentPubkey: AGENT, channelId: "chan-2", live: true },
  ]);
  assert.equal(published.length, 1);
  await act(async () => root.unmount());
});

test("an archived-only view never sends", async () => {
  reset();
  const root = createRoot(document.createElement("div"));
  // A stopped agent's panel shows archived history only.
  const live = isLiveSessionView(false, false);
  await render(root, [{ agentPubkey: AGENT, channelId: "chan-1", live }]);
  assert.equal(published.length, 0);
  assert.equal(isLiveSessionView(true, true), false, "E2E snapshot override");
  assert.equal(isLiveSessionView(true, false), true);
  await act(async () => root.unmount());
});

test("hiding the window releases the agent; showing it sends again", async () => {
  reset();
  const root = createRoot(document.createElement("div"));
  await render(root, [{ agentPubkey: AGENT, channelId: "chan-1", live: true }]);
  assert.equal(published.length, 1);

  visibility = "hidden";
  await act(async () => {
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await flush();
  assert.deepEqual(getObserverWatchingRegistry().heldAgents(), []);
  assert.equal(published.length, 1);

  visibility = "visible";
  await act(async () => {
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await flush();
  assert.equal(published.length, 2);
  await act(async () => root.unmount());
});

test("a reconnect sends at once for a held agent", async () => {
  reset();
  const root = createRoot(document.createElement("div"));
  await render(root, [{ agentPubkey: AGENT, channelId: "chan-1", live: true }]);
  assert.equal(published.length, 1);
  for (const state of ["reconnecting", "connected"]) {
    connection = state;
    for (const listener of connectionListeners) listener(state);
  }
  await flush();
  assert.equal(published.length, 2);
  await act(async () => root.unmount());
});
