import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

// The startup error screens (connection failed, not a member) with a built-in
// server: Retry is always offered; Sign out only with a signed-in session, and
// it signs out on this computer only (`logout_on_this_device`), never the
// network sign-out (`logout`) or the full local reset.

globalThis.window.matchMedia ??= () => ({
  matches: false,
  addEventListener() {},
  removeEventListener() {},
  addListener() {},
  removeListener() {},
});

const calls = [];
let accounts = [];
const tauriMock = {
  invoke(command, args) {
    calls.push({ command, args });
    switch (command) {
      case "list_accounts":
        return Promise.resolve(accounts);
      case "logout_on_this_device":
        return Promise.resolve({
          origin: "https://jihobuzz.duckdns.org:8443",
          supported: true,
          providers: ["google"],
          state: "signed_out",
          principal: null,
          deviceId: null,
          reason: null,
          deviceSignedOut: true,
          switchedTo: null,
          agentsNotStopped: 0,
        });
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

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { identityReload } = await import("../../../shared/api/tokenAuth.ts");
const { setBuiltInServerForTests } = await import("../builtInServer.ts");
const { CommunityApplyErrorScreen } = await import(
  "./CommunityApplyErrorScreen.tsx"
);
const { MembershipDenied } = await import(
  "../../onboarding/ui/MembershipDenied.tsx"
);
const { CommunityOnboardingProvider } = await import(
  "../../onboarding/communityOnboarding.tsx"
);

const BUILT_IN = "wss://jihobuzz.duckdns.org:8443";
let reloads = 0;
identityReload.reload = () => {
  reloads += 1;
};

const signedIn = [
  {
    pubkey: "a".repeat(64),
    origin: "https://jihobuzz.duckdns.org:8443",
    state: "signed_in",
    active: true,
    displayName: null,
    buzzName: null,
    avatarUrl: null,
    email: null,
    lastUsedAt: null,
  },
];

let mounted = null;

afterEach(async () => {
  if (mounted) {
    await act(async () => mounted.root.unmount());
    mounted.container.remove();
  }
  mounted = null;
  calls.length = 0;
  reloads = 0;
  setBuiltInServerForTests(undefined);
});

async function mount(element) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => root.render(element));
  for (let i = 0; i < 5; i += 1) {
    await act(() => new Promise((resolve) => setTimeout(resolve, 5)));
  }
  mounted = { container, root };
  return container;
}

function applyErrorScreen(onRetry = () => {}) {
  return React.createElement(CommunityApplyErrorScreen, {
    error: "Couldn't reach Kuzz.",
    onRetry,
  });
}

test("connection error: Retry always; Sign out signs out on this computer", async () => {
  accounts = signedIn;
  let retries = 0;
  const container = await mount(applyErrorScreen(() => (retries += 1)));
  const retry = container.querySelector(
    "[data-testid='community-apply-error-retry']",
  );
  assert.ok(retry, "Retry shown");
  await act(async () => retry.click());
  assert.equal(retries, 1);

  const signOut = container.querySelector(
    "[data-testid='community-apply-error-sign-out']",
  );
  assert.ok(signOut, "Sign out shown with a signed-in session");
  await act(async () => signOut.click());
  await act(() => new Promise((resolve) => setTimeout(resolve, 5)));
  const commands = calls.map((call) => call.command);
  assert.ok(commands.includes("logout_on_this_device"));
  assert.equal(commands.includes("logout"), false, "no network sign-out");
  assert.equal(reloads, 1, "restarts on the signed-out screen");
});

test("connection error without a signed-in session offers only Retry", async () => {
  for (const without of [
    [],
    [{ ...signedIn[0], state: "signed_out" }],
    [{ ...signedIn[0], active: false }],
  ]) {
    accounts = without;
    const container = await mount(applyErrorScreen());
    assert.ok(
      container.querySelector("[data-testid='community-apply-error-retry']"),
    );
    assert.equal(
      container.querySelector("[data-testid='community-apply-error-sign-out']"),
      null,
      JSON.stringify(without),
    );
    await act(async () => mounted.root.unmount());
    mounted.container.remove();
    mounted = null;
  }
});

function membershipDenied() {
  return React.createElement(
    CommunityOnboardingProvider,
    { enabled: false },
    React.createElement(MembershipDenied, {
      activeRelayUrl: BUILT_IN,
      onBack: () => {},
      onImportKey: async () => {},
      onRetry: () => {},
    }),
  );
}

test("not a member: switching account signs out on this computer only", async () => {
  setBuiltInServerForTests(BUILT_IN);
  accounts = signedIn;
  const container = await mount(membershipDenied());
  assert.ok(
    [...container.querySelectorAll("button")].some(
      (button) => button.textContent.trim() === "Try again",
    ),
    "Retry shown",
  );
  const switchAccount = container.querySelector(
    "[data-testid='membership-denied-switch-account']",
  );
  assert.ok(switchAccount);
  await act(async () => switchAccount.click());
  await act(() => new Promise((resolve) => setTimeout(resolve, 5)));
  const commands = calls.map((call) => call.command);
  assert.ok(commands.includes("logout_on_this_device"));
  assert.equal(commands.includes("logout"), false, "no network sign-out");
});

test("not a member without a signed-in session: no sign-out, Retry stays", async () => {
  setBuiltInServerForTests(BUILT_IN);
  accounts = [];
  const container = await mount(membershipDenied());
  assert.ok(
    [...container.querySelectorAll("button")].some(
      (button) => button.textContent.trim() === "Try again",
    ),
  );
  assert.equal(
    container.querySelector("[data-testid='membership-denied-switch-account']"),
    null,
  );
});
