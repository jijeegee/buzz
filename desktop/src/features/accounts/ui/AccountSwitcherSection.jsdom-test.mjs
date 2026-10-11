import assert from "node:assert/strict";
import test from "node:test";

// The profile menu's account list bound to the real hook and the real Tauri
// command names: most-recent order, the active account checked, a switch
// reloads once with its overlay caption, an ended sign-in and "Add account"
// both open Google sign-in.

// The sign-in progress toast schedules frames.
globalThis.requestAnimationFrame ??= (callback) => setTimeout(callback, 0);

const calls = [];
let accounts = [];
let switchBehavior;
let loginBehavior;
const callbacks = new Map();

const tauriMock = {
  invoke(command, args) {
    calls.push({ command, args });
    switch (command) {
      case "list_accounts":
        return Promise.resolve(accounts);
      case "switch_account":
        return switchBehavior ? switchBehavior(args) : Promise.resolve(null);
      case "login_with_google":
        return loginBehavior ? loginBehavior(args) : new Promise(() => {});
      case "cancel_google_login":
        return Promise.resolve(true);
      case "plugin:event|listen":
        return Promise.resolve(1);
      case "plugin:event|unlisten":
        return Promise.resolve(null);
      default:
        return Promise.reject(new Error(`unmocked Tauri command: ${command}`));
    }
  },
  transformCallback(callback) {
    const id = Math.random();
    callbacks.set(id, callback);
    return id;
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
const { QueryClient, QueryClientProvider } = await import(
  "@tanstack/react-query"
);
const { AccountSwitcherSection } = await import("./AccountSwitcherSection.tsx");
const { useAccountMenu } = await import("../useAccountMenu.ts");
const tokenAuthApi = await import("../../../shared/api/tokenAuth.ts");
const accountSwitch = await import("../lib/accountSwitch.ts");

const me = "a".repeat(64);
const work = "b".repeat(64);
const ended = "c".repeat(64);
const gone = "d".repeat(64);

function account(pubkey, overrides = {}) {
  return {
    pubkey,
    origin: "http://127.0.0.1:3000",
    state: "signed_in",
    active: false,
    displayName: null,
    avatarUrl: null,
    email: null,
    lastUsedAt: null,
    ...overrides,
  };
}

function Harness() {
  const menu = useAccountMenu({ open: true });
  return React.createElement(AccountSwitcherSection, {
    accounts: menu.accounts,
    busy: menu.busy,
    onSignIn: () => void menu.signIn(me),
    onSwitch: (target) => void menu.switchTo(target),
  });
}

async function settle(predicate) {
  for (let i = 0; i < 40 && !predicate(); i += 1) {
    await act(() => new Promise((resolve) => setTimeout(resolve, 5)));
  }
}

async function mount() {
  calls.length = 0;
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  await act(async () => {
    root.render(
      React.createElement(
        QueryClientProvider,
        { client: queryClient },
        React.createElement(Harness),
      ),
    );
  });
  await settle(
    () => container.querySelector("[data-testid='account-add']") !== null,
  );
  await settle(() => container.querySelectorAll("button").length > 1);
  return {
    container,
    row: (pubkey) =>
      container.querySelector(`[data-testid='account-row-${pubkey}']`),
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

function standardAccounts() {
  return [
    account(me, {
      active: true,
      displayName: "Jiho",
      email: "jiho@example.com",
    }),
    account(work, { displayName: "Work" }),
    account(ended, { state: "needs_login", email: "old@example.com" }),
    account(gone, { state: "signed_out", displayName: "Gone" }),
  ];
}

test("the list keeps the backend's order, checks the active account and hides signed-out ones", async () => {
  accounts = standardAccounts();
  const { container, row, unmount } = await mount();
  const rows = [...container.querySelectorAll("[data-testid^='account-row-']")];
  assert.deepEqual(
    rows.map((r) => r.dataset.testid.slice("account-row-".length)),
    [me, work, ended],
  );
  assert.equal(row(gone), null);
  assert.equal(row(me).getAttribute("aria-current"), "true");
  assert.ok(row(me).querySelector("[aria-label='Current account']"));
  assert.equal(row(work).querySelector("[aria-label='Current account']"), null);
  assert.match(row(me).textContent, /Jiho.*jiho@example\.com/);
  assert.match(row(ended).textContent, /old@example\.com.*Sign in again/);
  assert.doesNotMatch(container.textContent, /\bkey\b|nsec|npub|relay/i);
  await unmount();
});

test("choosing another account switches once and reloads with its name", async (t) => {
  const reload = t.mock.method(tokenAuthApi.identityReload, "reload", () => {});
  accounts = standardAccounts();
  const { row, unmount } = await mount();
  await act(async () => fireEvent.click(row(me)));
  assert.equal(
    calls.some((c) => c.command === "switch_account"),
    false,
    "the active account is not switched to",
  );
  await act(async () => fireEvent.click(row(work)));
  await settle(() => reload.mock.callCount() > 0);
  assert.deepEqual(
    calls.filter((c) => c.command === "switch_account").map((c) => c.args),
    [{ pubkey: work }],
  );
  assert.equal(reload.mock.callCount(), 1);
  accountSwitch.takeAccountSwitchAtBoot();
  assert.equal(accountSwitch.accountSwitchCaption(), "Switching to Work…");
  assert.equal(accountSwitch.finishAccountSwitchBoot(), null);
  assert.equal(accountSwitch.accountSwitchCaption(), null, "caption is done");
  await unmount();
});

test("a switch the backend could not finish everywhere leaves one notice", async (t) => {
  const reload = t.mock.method(tokenAuthApi.identityReload, "reload", () => {});
  accounts = standardAccounts();
  switchBehavior = () => Promise.resolve("hand-over failed: 503");
  const { row, unmount } = await mount();
  await act(async () => fireEvent.click(row(work)));
  await settle(() => reload.mock.callCount() > 0);
  accountSwitch.takeAccountSwitchAtBoot();
  const notice = accountSwitch.finishAccountSwitchBoot();
  assert.match(notice, /Switched accounts/);
  assert.doesNotMatch(notice, /503|hand-over/, "no internal error text");
  assert.equal(accountSwitch.finishAccountSwitchBoot(), null, "only once");
  switchBehavior = undefined;
  await unmount();
});

test("a failed switch stays on the current account", async (t) => {
  const reload = t.mock.method(tokenAuthApi.identityReload, "reload", () => {});
  accounts = standardAccounts();
  switchBehavior = () => Promise.reject("This account is not on this computer.");
  const { row, unmount } = await mount();
  await act(async () => fireEvent.click(row(work)));
  await settle(() => !row(work).disabled);
  assert.equal(reload.mock.callCount(), 0);
  assert.equal(window.sessionStorage.getItem("buzz:switching-to"), null);
  switchBehavior = undefined;
  await unmount();
});

test("an ended sign-in and Add account both open Google sign-in; a new account reloads", async (t) => {
  const reload = t.mock.method(tokenAuthApi.identityReload, "reload", () => {});
  accounts = standardAccounts();
  const { container, row, unmount } = await mount();
  await act(async () => fireEvent.click(row(ended)));
  await settle(() => calls.some((c) => c.command === "login_with_google"));
  assert.equal(
    calls.find((c) => c.command === "login_with_google").args.connect,
    undefined,
    "never links the current account to another Google account",
  );
  assert.ok(
    container.querySelector("[data-testid='account-add']").disabled,
    "one sign-in at a time",
  );
  await unmount();

  let finish;
  loginBehavior = () =>
    new Promise((resolve) => {
      finish = resolve;
    });
  const second = await mount();
  await act(async () =>
    fireEvent.click(
      second.container.querySelector("[data-testid='account-add']"),
    ),
  );
  await settle(() => finish !== undefined);
  await act(async () =>
    finish({
      origin: "http://127.0.0.1:3000",
      supported: true,
      providers: ["google"],
      state: "active",
      principal: "e".repeat(64),
      deviceId: null,
      reason: null,
      keyBackup: true,
      signingPubkey: "f".repeat(64),
      accountChanged: true,
    }),
  );
  await settle(() => reload.mock.callCount() > 0);
  assert.equal(reload.mock.callCount(), 1);
  accountSwitch.takeAccountSwitchAtBoot();
  assert.equal(accountSwitch.accountSwitchCaption(), "Switching accounts…");
  accountSwitch.resetAccountSwitchForTests();
  loginBehavior = undefined;
  await second.unmount();
});
