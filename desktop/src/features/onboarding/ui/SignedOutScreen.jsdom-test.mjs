import assert from "node:assert/strict";
import test from "node:test";

// App.tsx mounts the community app through SignedOutGate. While the native
// status says this computer is signed out, the gate renders the signed-out
// screen and the chat shell (and its relay connection) never mounts.

const relayUrl = "wss://relay.test";
const calls = [];
let runtimes = [];
let supported = true;

const tauriMock = {
  invoke(command, args) {
    calls.push({ command, args });
    switch (command) {
      case "get_token_auth_status":
        return Promise.resolve({
          origin: "https://relay.test",
          supported,
          providers: supported ? ["google"] : [],
          state: "signed_out",
          principal: null,
          deviceId: null,
          reason: null,
          keyBackup: true,
          keyBackupSupported: true,
          deviceSignedOut: true,
        });
      case "list_managed_agent_runtimes":
        return Promise.resolve(runtimes);
      case "plugin:event|listen":
        return Promise.resolve(1);
      case "plugin:event|unlisten":
        return Promise.resolve(null);
      default:
        return Promise.reject(new Error(`unmocked Tauri command: ${command}`));
    }
  },
  transformCallback() {
    return Math.random();
  },
  unregisterCallback() {},
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_INTERNALS__ = tauriMock;
globalThis.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
globalThis.window.__TAURI_EVENT_PLUGIN_INTERNALS__ =
  globalThis.__TAURI_EVENT_PLUGIN_INTERNALS__;
globalThis.window.matchMedia ??= () => ({
  matches: false,
  addEventListener() {},
  removeEventListener() {},
  addListener() {},
  removeListener() {},
});

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { SignedOutGate } = await import("./SignedOutScreen.tsx");

const chatShell = React.createElement(
  "div",
  { "data-testid": "chat-shell" },
  "Chats",
);

async function mount(signedOut) {
  calls.length = 0;
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () =>
    root.render(React.createElement(SignedOutGate, { signedOut }, chatShell)),
  );
  for (let i = 0; i < 20; i += 1) {
    await act(() => new Promise((resolve) => setTimeout(resolve, 5)));
  }
  return {
    container,
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

test("a signed-out computer shows the signed-out screen instead of chats", async () => {
  runtimes = [
    { pubkey: "b".repeat(64), relayUrl, lifecycle: "ready" },
    {
      pubkey: "b".repeat(64),
      relayUrl: "wss://other.test",
      lifecycle: "listening",
    },
    { pubkey: "c".repeat(64), relayUrl, lifecycle: "stopped" },
  ];
  const { container, unmount } = await mount(true);
  assert.equal(container.querySelector("[data-testid='chat-shell']"), null);
  const screen = container.querySelector("[data-testid='signed-out-screen']");
  assert.ok(screen, "signed-out screen rendered");
  assert.match(screen.textContent, /You're signed out/);
  const buttons = [...screen.querySelectorAll("button")];
  assert.deepEqual(
    buttons.map((button) => button.textContent.trim()),
    ["Sign in with Google"],
    "Sign in with Google is the only action; no agent controls",
  );
  assert.equal(
    screen.querySelector("[data-testid='signed-out-running-agents']")
      .textContent,
    "1 agent is still running on this computer.",
  );
  assert.doesNotMatch(screen.textContent, /\bkey\b|nsec|npub|relay/i);
  await act(async () => buttons[0].click());
  const login = calls.find((c) => c.command === "login_with_google");
  assert.equal(login?.args.allowRestore, true, "restore-mode Google sign-in");
  await unmount();
});

test("several running agents are counted; none are not mentioned", async () => {
  runtimes = [
    { pubkey: "b".repeat(64), relayUrl, lifecycle: "ready" },
    { pubkey: "c".repeat(64), relayUrl, lifecycle: "starting" },
  ];
  let { container, unmount } = await mount(true);
  assert.equal(
    container.querySelector("[data-testid='signed-out-running-agents']")
      .textContent,
    "2 agents are still running on this computer.",
  );
  await unmount();

  runtimes = [];
  ({ container, unmount } = await mount(true));
  assert.ok(container.querySelector("[data-testid='signed-out-screen']"));
  assert.doesNotMatch(container.textContent, /agent/i);
  await unmount();
});

test("a signed-in computer mounts the chat shell", async () => {
  const { container, unmount } = await mount(false);
  assert.ok(container.querySelector("[data-testid='chat-shell']"));
  assert.equal(
    container.querySelector("[data-testid='signed-out-screen']"),
    null,
  );
  assert.equal(calls.length, 0, "no native calls from the gate itself");
  await unmount();
});

test("Google being unreachable still leaves a way to retry", async () => {
  supported = false;
  runtimes = [];
  const { container, unmount } = await mount(true);
  assert.match(container.textContent, /Google sign-in isn't available/);
  assert.ok(
    [...container.querySelectorAll("button")].some(
      (button) => button.textContent.trim() === "Try again",
    ),
  );
  supported = true;
  await unmount();
});
