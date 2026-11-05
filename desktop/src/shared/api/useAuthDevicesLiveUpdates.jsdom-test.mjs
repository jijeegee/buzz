import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

// Launch draws the stored account before its session is restored: the device
// list fails first, and must come back as soon as the session does.

const THIS_PC = { id: "this-pc", name: "This PC", current: true, agents: [] };

let listDevices;
let tokenListeners = [];
const tauriMock = {
  invoke(command, args) {
    switch (command) {
      case "list_devices":
        return listDevices();
      case "plugin:event|listen":
        tokenListeners.push(args.handler);
        return Promise.resolve(tokenListeners.length);
      case "plugin:event|unlisten":
        return Promise.resolve();
      default:
        return Promise.reject(new Error(`unmocked Tauri command: ${command}`));
    }
  },
  transformCallback(callback) {
    return callback;
  },
  unregisterCallback() {},
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
  unregisterListener() {},
};

const { act, cleanup, render, waitFor } = await import(
  "@testing-library/react"
);
const { QueryClient, QueryClientProvider } = await import(
  "@tanstack/react-query"
);
const React = (await import("react")).default;
const {
  authDevicesRetryDelay,
  useAuthDevicesLiveUpdates,
  useAuthDevicesQuery,
} = await import("./useAuthDeviceNames.ts");

afterEach(() => {
  cleanup();
  tokenListeners = [];
});

function Harness({ onDevices }) {
  useAuthDevicesLiveUpdates();
  onDevices(useAuthDevicesQuery().data);
  return null;
}

function mount(onDevices) {
  const client = new QueryClient();
  render(
    React.createElement(
      QueryClientProvider,
      { client },
      React.createElement(Harness, { onDevices }),
    ),
  );
  return client;
}

const failures = (client) =>
  client.getQueryState(["auth-devices"])?.fetchFailureCount ?? 0;

// Under the 1s first retry, so these reads cannot come from the retry.
const BEFORE_FIRST_RETRY_MS = 700;

test("retry waits grow to five minutes and stay there", () => {
  assert.deepEqual(
    [0, 1, 2, 3, 4, 5, 9].map(authDevicesRetryDelay),
    [1_000, 5_000, 30_000, 60_000, 300_000, 300_000, 300_000],
  );
});

test("a failed list is not cached as empty and returns once the session does", async () => {
  let signedIn = false;
  listDevices = () =>
    signedIn
      ? Promise.resolve([THIS_PC])
      : Promise.reject(new Error("not signed in with Google"));
  let latest = "unset";
  const client = mount((devices) => {
    latest = devices;
  });
  await waitFor(() => {
    assert.equal(failures(client), 1);
    assert.equal(tokenListeners.length, 1);
  });
  assert.equal(latest, undefined);

  // At once, not after the first wait.
  signedIn = true;
  const start = Date.now();
  await act(async () => {
    tokenListeners[0]({ event: "token-auth-changed", payload: "origin" });
  });
  await waitFor(() => assert.deepEqual(latest, [THIS_PC]));
  assert.ok(Date.now() - start < BEFORE_FIRST_RETRY_MS);
  assert.equal(failures(client), 0);
});

test("a failure before the listener attached is read again when it attaches", async () => {
  let attached;
  const attachedOnce = new Promise((resolve) => {
    attached = resolve;
  });
  let calls = 0;
  listDevices = () => {
    calls++;
    if (calls > 1) return Promise.resolve([THIS_PC]);
    return Promise.reject(new Error("not signed in with Google"));
  };
  const client = new QueryClient();
  function First({ onDevices }) {
    onDevices(useAuthDevicesQuery().data);
    return null;
  }
  function Later() {
    useAuthDevicesLiveUpdates();
    attached();
    return null;
  }
  let latest = "unset";
  const tree = (later) =>
    React.createElement(
      QueryClientProvider,
      { client },
      React.createElement(First, {
        onDevices: (devices) => {
          latest = devices;
        },
      }),
      later ? React.createElement(Later) : null,
    );
  const view = render(tree(false));
  await waitFor(() => assert.equal(failures(client), 1));
  const start = Date.now();
  view.rerender(tree(true));
  await attachedOnce;
  await waitFor(() => assert.deepEqual(latest, [THIS_PC]));
  assert.ok(Date.now() - start < BEFORE_FIRST_RETRY_MS);
  assert.equal(calls, 2);
});

test("a later failure keeps the list already read", async () => {
  listDevices = () => Promise.resolve([THIS_PC]);
  let latest = "unset";
  const client = mount((devices) => {
    latest = devices;
  });
  await waitFor(() => assert.deepEqual(latest, [THIS_PC]));

  listDevices = () => Promise.reject(new Error("offline"));
  await act(async () => {
    void client.invalidateQueries({ queryKey: ["auth-devices"] });
  });
  await waitFor(() => assert.equal(failures(client), 1));
  assert.deepEqual(latest, [THIS_PC]);
});
