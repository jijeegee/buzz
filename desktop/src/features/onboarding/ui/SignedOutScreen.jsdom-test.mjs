import assert from "node:assert/strict";
import test from "node:test";

// App.tsx mounts the community app through SignedOutGate. While the native
// status says this computer is signed out, the gate renders the signed-out
// screen and the chat shell (and its relay connection) never mounts.

const relayUrl = "wss://relay.test";
const calls = [];
let runtimes = [];
let supported = true;
let accounts = [];

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
      case "list_accounts":
        return Promise.resolve(accounts);
      case "switch_account":
        return Promise.resolve(null);
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
    ["Sign in with Google", "Terms of Service"],
    "Sign in with Google is the only action (plus the terms link); no agent controls",
  );
  // The signed-out account's agents stopped with it: none are reported.
  assert.doesNotMatch(screen.textContent, /agent/i);
  assert.doesNotMatch(screen.textContent, /\bkey\b|nsec|npub|relay/i);
  await act(async () => buttons[0].click());
  const login = calls.find((c) => c.command === "login_with_google");
  assert.equal(login?.args.allowRestore, true, "restore-mode Google sign-in");
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

test("another account still signed in here is offered to switch to", async () => {
  const account = (pubkey, overrides) => ({
    pubkey,
    origin: "https://relay.test",
    state: "signed_in",
    active: false,
    displayName: null,
    kuzzName: null,
    avatarUrl: null,
    email: null,
    lastUsedAt: null,
    ...overrides,
  });
  accounts = [
    account("a".repeat(64), { active: true, state: "signed_out" }),
    account("b".repeat(64), { kuzzName: "Jiho" }),
    account("c".repeat(64), { state: "needs_login", kuzzName: "Old" }),
    account("d".repeat(64), { state: "key_only" }),
  ];
  const tokenAuth = await import("../../../shared/api/tokenAuth.ts");
  const reloads = [];
  const original = tokenAuth.identityReload.reload;
  tokenAuth.identityReload.reload = () => reloads.push(true);
  const { container, unmount } = await mount(true);
  const section = container.querySelector(
    "[data-testid='signed-out-other-accounts']",
  );
  assert.ok(section, "other signed-in accounts listed");
  // Only the signed-in one: not the active, needs-login or key-only accounts.
  assert.equal(section.querySelectorAll("button").length, 1);
  assert.match(section.textContent, /Continue as Jiho/);
  assert.doesNotMatch(section.textContent, /Old|Account /);
  await act(async () =>
    section
      .querySelector(`[data-testid='signed-out-switch-${"b".repeat(64)}']`)
      .click(),
  );
  for (let i = 0; i < 5; i += 1) {
    await act(() => new Promise((resolve) => setTimeout(resolve, 5)));
  }
  const switched = calls.find((c) => c.command === "switch_account");
  assert.equal(switched?.args.pubkey, "b".repeat(64));
  assert.equal(reloads.length, 1, "reloads into the chosen account");
  tokenAuth.identityReload.reload = original;
  accounts = [];
  await unmount();
});

test("no other signed-in account: only Google sign-in", async () => {
  accounts = [];
  const { container, unmount } = await mount(true);
  assert.equal(
    container.querySelector("[data-testid='signed-out-other-accounts']"),
    null,
  );
  await unmount();
});
