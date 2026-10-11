import assert from "node:assert/strict";
import test from "node:test";

// A Google account with no Buzz account yet: the native sign-in returns a
// `signup` prompt (the relay created nothing). The shared login hook shows
// "Create your Buzz account" and only "Create account" creates it; "Use a
// different account" starts a fresh Google sign-in and closing stops.

const calls = [];
/** Results `login_with_google` returns, in order. */
let logins = [];
let confirmResult;

const tauriMock = {
  invoke(command, args) {
    calls.push({ command, args });
    switch (command) {
      case "login_with_google":
        return Promise.resolve(logins.shift());
      case "confirm_signup":
        return Promise.resolve(confirmResult);
      case "cancel_signup":
      case "cancel_google_login":
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

const { fireEvent } = await import("@testing-library/react");
const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { useGoogleLogin } = await import("../hooks/useGoogleLogin.ts");
const { CreateAccountDialog } = await import("./CreateAccountDialog.tsx");

function status(overrides = {}) {
  return {
    origin: "https://relay.test",
    supported: true,
    providers: ["google"],
    state: "active",
    principal: "ab".repeat(32),
    deviceId: "d",
    reason: null,
    keyBackup: true,
    signingPubkey: "cd".repeat(32),
    ...overrides,
  };
}

const prompt = (email) =>
  status({
    state: "signed_out",
    principal: null,
    signup: { email, displayName: "New", avatarUrl: null },
  });

let outcome;
function Harness() {
  const login = useGoogleLogin();
  return React.createElement(
    "div",
    null,
    React.createElement(
      "button",
      {
        "data-testid": "start",
        type: "button",
        onClick: () => {
          outcome = login.run(false).then((result) => ({ result }));
        },
      },
      "Sign in",
    ),
    React.createElement(CreateAccountDialog, { login }),
  );
}

async function settle(predicate = () => false) {
  for (let i = 0; i < 40 && !predicate(); i += 1) {
    await act(() => new Promise((resolve) => setTimeout(resolve, 5)));
  }
}

async function mount() {
  calls.length = 0;
  outcome = undefined;
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => root.render(React.createElement(Harness)));
  return {
    container,
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

const dialog = () =>
  document.querySelector("[data-testid='create-account-dialog']");
const count = (command) => calls.filter((c) => c.command === command).length;

test("an existing account signs straight in without the screen", async () => {
  logins = [status()];
  const { container, unmount } = await mount();
  await act(async () =>
    fireEvent.click(container.querySelector("[data-testid='start']")),
  );
  const { result } = await outcome;
  assert.equal(result.state, "active");
  assert.equal(dialog(), null);
  assert.equal(count("confirm_signup"), 0);
  await unmount();
});

test("a new Google account is created only after Create account", async () => {
  logins = [prompt("new@example.test")];
  confirmResult = status();
  const { container, unmount } = await mount();
  await act(async () =>
    fireEvent.click(container.querySelector("[data-testid='start']")),
  );
  await settle(() => dialog() !== null);
  assert.match(dialog().textContent, /Create your Buzz account/);
  assert.equal(
    document.querySelector("[data-testid='create-account-email']").textContent,
    "new@example.test",
  );
  assert.doesNotMatch(dialog().textContent, /key|nsec|relay/i);
  assert.equal(count("confirm_signup"), 0, "nothing created while asking");

  await act(async () =>
    fireEvent.click(
      document.querySelector("[data-testid='create-account-confirm']"),
    ),
  );
  const { result } = await outcome;
  assert.equal(result.state, "active");
  assert.equal(count("confirm_signup"), 1);
  assert.equal(count("cancel_signup"), 0);
  await settle(() => dialog() === null);
  assert.equal(dialog(), null);
  await unmount();
});

test("Use a different account drops the new one and signs in again", async () => {
  logins = [prompt("wrong@example.test"), status()];
  const { container, unmount } = await mount();
  await act(async () =>
    fireEvent.click(container.querySelector("[data-testid='start']")),
  );
  await settle(() => dialog() !== null);
  await act(async () =>
    fireEvent.click(
      document.querySelector("[data-testid='create-account-different']"),
    ),
  );
  const { result } = await outcome;
  assert.equal(result.state, "active");
  assert.equal(count("cancel_signup"), 1);
  assert.equal(count("confirm_signup"), 0);
  assert.equal(count("login_with_google"), 2, "a fresh Google sign-in");
  await unmount();
});

test("closing the screen stops without creating anything", async () => {
  logins = [prompt("new@example.test")];
  const { container, unmount } = await mount();
  await act(async () =>
    fireEvent.click(container.querySelector("[data-testid='start']")),
  );
  await settle(() => dialog() !== null);
  await act(async () => fireEvent.keyDown(dialog(), { key: "Escape" }));
  const { result } = await outcome;
  assert.equal(result, null);
  assert.equal(count("cancel_signup"), 1);
  assert.equal(count("confirm_signup"), 0);
  await unmount();
});
