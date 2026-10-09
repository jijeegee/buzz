import assert from "node:assert/strict";
import test from "node:test";

// The first onboarding screen binds the real flow to the real Tauri commands:
// users only ever see Google sign-in (or a retryable error), never the
// signing-key paths, which exist only for E2E mock builds.

const calls = [];
let statusBehavior;
const tauriMock = {
  invoke(command) {
    calls.push(command);
    switch (command) {
      case "get_token_auth_status":
        return statusBehavior();
      case "plugin:event|listen":
        return Promise.resolve(1);
      case "plugin:event|unlisten":
        return Promise.resolve(null);
      case "cancel_google_login":
        return Promise.resolve(false);
      case "get_identity":
        return new Promise(() => {});
      default:
        return Promise.reject(new Error(`unmocked Tauri command: ${command}`));
    }
  },
  transformCallback: () => Math.random(),
  unregisterCallback() {},
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_INTERNALS__ = tauriMock;
globalThis.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
globalThis.window.__TAURI_EVENT_PLUGIN_INTERNALS__ =
  globalThis.__TAURI_EVENT_PLUGIN_INTERNALS__;
// The landing's decorative bees animate on frames; jsdom has none.
globalThis.requestAnimationFrame ??= () => 0;
globalThis.cancelAnimationFrame ??= () => {};
globalThis.window.requestAnimationFrame ??= globalThis.requestAnimationFrame;
globalThis.window.cancelAnimationFrame ??= globalThis.cancelAnimationFrame;
globalThis.window.matchMedia ??= () => ({
  matches: false,
  addEventListener() {},
  removeEventListener() {},
  addListener() {},
  removeListener() {},
});

const { fireEvent } = await import("@testing-library/react");
const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { QueryClient } = await import("@tanstack/react-query");
const { MachineOnboardingFlow } = await import("./MachineOnboardingFlow.tsx");

function status(overrides = {}) {
  return {
    origin: "https://relay.example",
    supported: true,
    providers: ["google"],
    state: "signed_out",
    principal: null,
    deviceId: null,
    reason: null,
    keyBackupSupported: true,
    ...overrides,
  };
}

async function settle(predicate) {
  for (let i = 0; i < 40 && !predicate(); i += 1) {
    await act(() => new Promise((resolve) => setTimeout(resolve, 5)));
  }
}

async function mount(props = {}) {
  calls.length = 0;
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  const noop = () => {};
  await act(async () =>
    root.render(
      React.createElement(MachineOnboardingFlow, {
        complete: noop,
        continueWithIdentity: noop,
        continueWithRecoveredIdentity: noop,
        identityLost: false,
        queryClient: new QueryClient(),
        ...props,
      }),
    ),
  );
  return {
    container,
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

const KEY_WORDING =
  /identity key|existing key|different key|private key|\bnsec\b|npub|Nostr|Continue setup/i;

test("the landing offers only Google sign-in, with no key wording", async () => {
  statusBehavior = () => Promise.resolve(status());
  const { container, unmount } = await mount();
  await settle(
    () =>
      container.querySelector("[data-testid='onboarding-google-sign-in']") !==
      null,
  );
  assert.ok(
    container.querySelector("[data-testid='onboarding-google-sign-in']"),
  );
  assert.match(
    container.textContent,
    /Sign in with your Google account to get started\./,
  );
  assert.doesNotMatch(container.textContent, KEY_WORDING);
  assert.doesNotMatch(container.textContent, /\bkey\b|signing/i);
  await unmount();
});

test("unreachable Google sign-in shows a plain error with Retry, never key options", async () => {
  statusBehavior = () => Promise.reject(new Error("offline"));
  const { container, unmount } = await mount();
  await settle(
    () =>
      container.querySelector("[data-testid='onboarding-google-retry']") !==
      null,
  );
  assert.match(container.textContent, /couldn’t reach Buzz sign-in/);
  assert.doesNotMatch(container.textContent, KEY_WORDING);

  statusBehavior = () => Promise.resolve(status());
  await act(async () =>
    fireEvent.click(
      container.querySelector("[data-testid='onboarding-google-retry']"),
    ),
  );
  await settle(
    () =>
      container.querySelector("[data-testid='onboarding-google-sign-in']") !==
      null,
  );
  assert.ok(
    container.querySelector("[data-testid='onboarding-google-sign-in']"),
  );
  await unmount();
});

test("a relay without Google sign-in is an error outside E2E builds", async () => {
  statusBehavior = () =>
    Promise.resolve(status({ supported: false, providers: [] }));
  const { container, unmount } = await mount();
  await settle(
    () =>
      container.querySelector("[data-testid='onboarding-google-retry']") !==
      null,
  );
  assert.ok(container.querySelector("[data-testid='onboarding-google-retry']"));
  assert.doesNotMatch(container.textContent, KEY_WORDING);
  await unmount();
});

test("an already signed-in Google session continues setup", async () => {
  statusBehavior = () =>
    Promise.resolve(status({ state: "active", keyBackup: true }));
  const { container, unmount } = await mount();
  await settle(
    () =>
      container.querySelector("[data-testid='onboarding-google-continue']") !==
      null,
  );
  const button = container.querySelector(
    "[data-testid='onboarding-google-continue']",
  );
  assert.equal(button.textContent, "Continue");
  await act(async () => fireEvent.click(button));
  assert.ok(calls.includes("get_identity"));
  await unmount();
});

test("a lost identity is restored through Google, not a key-import screen", async () => {
  statusBehavior = () =>
    Promise.resolve(status({ state: "active", keyBackup: true }));
  const { container, unmount } = await mount({ identityLost: true });
  await settle(
    () =>
      container.querySelector("[data-testid='onboarding-google-sign-in']") !==
      null,
  );
  assert.ok(
    container.querySelector("[data-testid='onboarding-google-sign-in']"),
    "Google sign-in stays offered over a surviving session",
  );
  assert.match(
    container.textContent,
    /Sign in with Google to restore your account\./,
  );
  assert.doesNotMatch(container.textContent, KEY_WORDING);
  await unmount();
});
