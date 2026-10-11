import assert from "node:assert/strict";
import test from "node:test";

// Settings › Account binds the real card to the real Tauri command names: a
// relay without token sign-in shows nothing, a signed-out community offers
// Google sign-in, and a signed-in one lists devices with one labelled
// sign-out button per other device (no duplicate stop for this device).

const calls = [];
let status;
let loginBehavior;
let cancelBehavior;
let logoutBehavior;
let runtimes = [];
let accounts = [];
/** Running agents owned by another account signed in on this computer. */
let foreignAgents = new Set();
const callbacks = new Map();
let progressCallback;
function progress(attemptId, phase) {
  callbacks.get(progressCallback)?.({ payload: { attemptId, phase } });
}
const devices = [
  {
    id: "11111111-1111-1111-1111-111111111111",
    name: "This Mac",
    platform: "desktop",
    lastSeenAt: null,
    current: true,
  },
  {
    id: "22222222-2222-2222-2222-222222222222",
    name: "Work PC",
    platform: "desktop",
    lastSeenAt: null,
    current: false,
  },
];

const tauriMock = {
  invoke(command, args) {
    calls.push({ command, args });
    switch (command) {
      case "get_token_auth_status":
        return Promise.resolve(status);
      case "take_token_auth_notices":
        return Promise.resolve([]);
      case "list_devices":
        return Promise.resolve(devices);
      case "revoke_device":
      case "revoke_other_sessions":
      case "revoke_all_bot_tokens":
        return Promise.resolve(null);
      case "rename_device": {
        const device = devices.find((d) => d.id === args.deviceId);
        device.name = args.name;
        return Promise.resolve({ ...device });
      }
      case "cancel_google_login":
        return cancelBehavior ? cancelBehavior(args) : Promise.resolve(false);
      case "login_with_google":
        progress(args.attemptId, "waiting");
        return loginBehavior ? loginBehavior(args) : new Promise(() => {});
      case "logout":
        // Never settles by default: the real flow reloads afterwards.
        return logoutBehavior ? logoutBehavior(args) : new Promise(() => {});
      case "list_managed_agent_runtimes":
        return Promise.resolve(runtimes);
      case "list_accounts":
        return Promise.resolve(accounts);
      case "own_agent_pubkeys":
        // Agents of another account on this computer are never offered.
        return Promise.resolve(
          args.pubkeys.filter((pubkey) => !foreignAgents.has(pubkey)),
        );
      case "stop_managed_agent_runtime":
        return Promise.resolve({ ...args, lifecycle: "stopped" });
      case "plugin:event|listen":
        if (args.event === "google-login-progress")
          progressCallback = args.handler;
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
const { AccountSettingsCard } = await import("./AccountSettingsCard.tsx");
const tokenAuthApi = await import("../../../shared/api/tokenAuth.ts");
const accountSwitch = await import("../../accounts/lib/accountSwitch.ts");
const { deviceRobotTag } = await import("../../../shared/lib/deviceRobot.ts");
const { GoogleSignInButton } = await import(
  "../../onboarding/ui/GoogleSignInButton.tsx"
);

function baseStatus(overrides) {
  return {
    origin: "http://127.0.0.1:3000",
    supported: true,
    providers: ["google"],
    state: "signed_out",
    principal: null,
    deviceId: null,
    reason: null,
    ...overrides,
  };
}

async function settle(predicate) {
  for (let i = 0; i < 40 && !predicate(); i += 1) {
    await act(() => new Promise((resolve) => setTimeout(resolve, 5)));
  }
}

async function mount(Component = AccountSettingsCard, props) {
  calls.length = 0;
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  // Device rows read the owner's device records from the query cache.
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  await act(async () => {
    root.render(
      React.createElement(
        QueryClientProvider,
        { client: queryClient },
        React.createElement(Component, props),
      ),
    );
  });
  await settle(() => calls.some((c) => c.command === "get_token_auth_status"));
  await act(() => new Promise((resolve) => setTimeout(resolve, 10)));
  return {
    container,
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

test("a relay without token sign-in renders nothing", async () => {
  status = baseStatus({ supported: false, providers: [] });
  const { container, unmount } = await mount();
  assert.equal(
    container.querySelector("[data-testid='settings-account']"),
    null,
  );
  await unmount();
});

test("account deletion is collapsed and opening or cancelling never deletes", async () => {
  status = baseStatus({ state: "active", principal: "ab".repeat(32) });
  const { container, unmount } = await mount();
  const details = container.querySelector("details");
  assert.equal(details.open, false);
  await act(async () => fireEvent.click(details.querySelector("summary")));
  assert.equal(details.open, true);
  await act(async () => fireEvent.click(details.querySelector("button")));
  const dialog = document.querySelector('[role="alertdialog"]');
  assert.ok(dialog);
  assert.equal(
    calls.some((c) => c.command === "delete_account"),
    false,
  );
  const cancel = [...dialog.querySelectorAll("button")].find(
    (b) => b.textContent === "Cancel",
  );
  await act(async () => fireEvent.click(cancel));
  assert.equal(document.querySelector('[role="alertdialog"]'), null);
  assert.equal(
    calls.some((c) => c.command === "delete_account"),
    false,
  );
  await unmount();
});

test("a signed-out community offers Google sign-in through login_with_google", async () => {
  status = baseStatus();
  const { container, unmount } = await mount();
  const button = container.querySelector("[data-testid='account-sign-in']");
  assert.ok(button, "sign-in button rendered");
  assert.equal(button.textContent.trim(), "Sign in with Google");
  await act(async () => {
    fireEvent.click(button);
  });
  assert.equal(
    calls.filter((c) => c.command === "login_with_google").length,
    1,
    "exactly one login per click",
  );
  await settle(() => button.textContent.includes("Signing"));
  assert.ok(button.disabled, "no second login while one is in flight");
  // Rule 7: the button's own text is the only announcement; the spinner is
  // decorative and adds no nested status region or screen-reader stop.
  assert.equal(button.querySelector("[role='status']"), null);
  const spinner = button.querySelector(".sprout-arc-spinner");
  assert.ok(spinner, "spinner rendered");
  assert.equal(spinner.getAttribute("aria-hidden"), "true");
  assert.equal(button.querySelector(".sr-only"), null);
  await unmount();
});

test("a signed-in community lists devices with one labelled sign-out per other device", async () => {
  status = baseStatus({ state: "active", principal: "ab".repeat(32) });
  const { container, unmount } = await mount();
  await settle(() => container.querySelector("ul[aria-labelledby]") !== null);
  const list = container.querySelector("ul[aria-labelledby]");
  assert.ok(list, "device list is labelled by its heading");
  const buttons = [...list.querySelectorAll("button")].filter((b) =>
    b.getAttribute("aria-label")?.startsWith("Sign out"),
  );
  assert.equal(buttons.length, 1, "the current device has no sign-out button");
  assert.equal(buttons[0].getAttribute("aria-label"), "Sign out Work PC");
  // Each device shows its robot (the one its agents show their owner).
  const robots = [...list.querySelectorAll("[data-testid=device-robot-icon]")];
  assert.deepEqual(
    robots.map((robot) => robot.getAttribute("data-robot-tag")),
    devices.map((device) => deviceRobotTag(device.id)),
  );
  assert.ok(robots.every((robot) => robot.getAttribute("aria-hidden")));
  await act(async () => {
    fireEvent.click(buttons[0]);
  });
  await settle(() => calls.some((c) => c.command === "revoke_device"));
  assert.deepEqual(calls.find((c) => c.command === "revoke_device").args, {
    deviceId: devices[1].id,
  });
  await unmount();
});

test("any device can be renamed; the saved name is trimmed and Escape cancels", async () => {
  status = baseStatus({ state: "active", principal: "ab".repeat(32) });
  const { container, unmount } = await mount();
  await settle(() => container.querySelector("ul[aria-labelledby]") !== null);
  const rename = (label) =>
    container.querySelector(`button[aria-label='Rename ${label}']`);
  assert.ok(rename("This Mac"), "this device is renamable");
  assert.ok(rename("Work PC"), "other devices are renamable");

  await act(async () => fireEvent.click(rename("Work PC")));
  let input = container.querySelector(
    "input[aria-label='New name for Work PC']",
  );
  await act(async () => fireEvent.keyDown(input, { key: "Escape" }));
  assert.equal(container.querySelector("input"), null, "Escape cancels");

  await act(async () => fireEvent.click(rename("Work PC")));
  input = container.querySelector("input[aria-label='New name for Work PC']");
  await act(async () =>
    fireEvent.change(input, { target: { value: "  Office PC  " } }),
  );
  await act(async () => fireEvent.submit(input.closest("form")));
  await settle(() => calls.some((c) => c.command === "rename_device"));
  assert.deepEqual(calls.find((c) => c.command === "rename_device").args, {
    deviceId: devices[1].id,
    name: "Office PC",
  });
  await settle(() => container.textContent.includes("Office PC"));
  assert.ok(rename("Office PC"), "list shows the new name");
  devices[1].name = "Work PC";
  await unmount();
});

test("sign-out stays reachable while a sign-in restore is stuck (Rule 6)", async () => {
  status = baseStatus({ state: "restoring" });
  const { container, unmount } = await mount();
  const signIn = container.querySelector("[data-testid='account-sign-in']");
  assert.ok(signIn.disabled, "sign-in waits for the restore");
  const signOut = container.querySelector("[data-testid='account-sign-out']");
  assert.ok(signOut, "sign-out rendered while restoring");
  assert.equal(signOut.disabled, false);
  await act(async () => {
    fireEvent.click(signOut);
  });
  assert.equal(calls.filter((c) => c.command === "logout").length, 1);
  await unmount();
});

test("a local-only account is invited to protect itself with Google, in plain words", async () => {
  status = baseStatus({ keyBackupSupported: true });
  const { container, unmount } = await mount();
  const button = container.querySelector("[data-testid='account-sign-in']");
  assert.equal(button.textContent.trim(), "Continue with Google");
  assert.match(container.textContent, /Protect your account/);
  assert.match(container.textContent, /protected copy/);
  assert.doesNotMatch(container.textContent, /bkeyb|signing|nostr|npub/i);
  assert.equal(
    container.querySelector("[data-testid='account-sign-out']"),
    null,
    "a local-only account offers no sign-out",
  );
  await act(async () => fireEvent.click(button));
  // Protecting a local-only account links its own key, never another account.
  assert.deepEqual(calls.find((c) => c.command === "login_with_google").args, {
    allowRestore: false,
    connect: true,
    attemptId: calls.find((c) => c.command === "login_with_google").args
      .attemptId,
  });
  await unmount();
});

test("Google backup sessions do not offer token-agent revocation or identity deletion", async () => {
  status = baseStatus({
    state: "active",
    principal: "ab".repeat(32),
    keyBackup: true,
    keyBackupSupported: true,
    signingPubkey: "cd".repeat(32),
  });
  const { container, unmount } = await mount();
  assert.match(container.textContent, /Signed in with Google/);
  // Another Google account is added from the profile menu, not checked here.
  assert.doesNotMatch(container.textContent, /Check Google sign-in/);
  assert.match(
    container.textContent,
    /Switch or add accounts from your profile menu\./,
  );
  assert.doesNotMatch(container.textContent, /bkeyb|signing|backup/i);
  assert.doesNotMatch(
    container.textContent,
    /Disconnect all agents|Delete account/,
  );
  await unmount();
});

test("new-device Google onboarding explicitly requests same-key recovery", async () => {
  status = baseStatus({ keyBackupSupported: true });
  const { container, unmount } = await mount(GoogleSignInButton, {
    allowRestore: true,
  });
  const button = container.querySelector(
    "[data-testid='onboarding-google-sign-in']",
  );
  assert.ok(button);
  assert.match(
    container.textContent,
    /Sign in with your Google account to get started\./,
  );
  assert.doesNotMatch(container.textContent, /bkeyb|signing/i);
  await act(async () => fireEvent.click(button));
  assert.deepEqual(calls.find((c) => c.command === "login_with_google").args, {
    allowRestore: true,
    attemptId: calls.find((c) => c.command === "login_with_google").args
      .attemptId,
  });
  assert.equal(button.disabled, true);
  await unmount();
});

test("previously signed-out token users have an explicit existing-account route", async () => {
  for (const Component of [AccountSettingsCard, GoogleSignInButton]) {
    status = baseStatus({ keyBackupSupported: true });
    const { container, unmount } = await mount(Component);
    const button = container.querySelector(
      "[data-testid='existing-token-sign-in']",
    );
    assert.ok(
      button,
      "existing account recovery remains reachable before mode is known",
    );
    assert.equal(button.textContent, "Sign in to an older Buzz account");
    await act(async () => fireEvent.click(button));
    assert.deepEqual(
      calls.find((c) => c.command === "login_with_google").args,
      {
        allowRestore: false,
        existingTokenAccount: true,
        attemptId: calls.find((c) => c.command === "login_with_google").args
          .attemptId,
      },
    );
    await unmount();
  }
});

test("cancel ends the native wait and onboarding can immediately retry", async () => {
  status = baseStatus();
  let rejectLogin;
  loginBehavior = () =>
    new Promise((_, reject) => {
      rejectLogin = reject;
    });
  cancelBehavior = () => {
    rejectLogin("Google sign-in cancelled.");
    return Promise.resolve(true);
  };
  const { container, unmount } = await mount(GoogleSignInButton);
  try {
    const button = container.querySelector(
      "[data-testid='onboarding-google-sign-in']",
    );
    await act(async () => fireEvent.click(button));
    const first = calls.find((c) => c.command === "login_with_google").args
      .attemptId;
    assert.ok(first);
    const cancel = [...container.querySelectorAll("button")].find(
      (b) => b.textContent === "Cancel sign-in",
    );
    assert.ok(cancel);
    await act(async () => fireEvent.click(cancel));
    assert.equal(button.disabled, false);
    assert.equal(container.querySelector("[role='alert']"), null);
    assert.equal(
      calls.find((c) => c.command === "cancel_google_login").args.attemptId,
      first,
    );
    await act(async () => fireEvent.click(button));
    const second = calls.filter((c) => c.command === "login_with_google").at(-1)
      .args.attemptId;
    assert.notEqual(first, second);
    await act(async () => progress(first, "completing"));
    assert.match(container.textContent, /Cancel sign-in/);
    await act(async () => progress(second, "completing"));
    assert.doesNotMatch(container.textContent, /Cancel sign-in/);
    assert.match(container.textContent, /Finishing sign-in in Buzz/);
  } finally {
    await unmount();
    loginBehavior = cancelBehavior = undefined;
  }
});

test("native failure stays visible and releases the settings sign-in button", async () => {
  status = baseStatus();
  loginBehavior = () => Promise.reject("sign-in timed out");
  const { container, unmount } = await mount();
  try {
    const button = container.querySelector("[data-testid='account-sign-in']");
    await act(async () => fireEvent.click(button));
    assert.equal(button.disabled, false);
    assert.match(
      container.querySelector("[role='alert']").textContent,
      /timed out/,
    );
  } finally {
    await unmount();
    loginBehavior = undefined;
  }
});

test("completion winning cancellation finishes without restarting Google", async () => {
  status = baseStatus();
  let resolveLogin;
  let completed = 0;
  loginBehavior = () =>
    new Promise((resolve) => {
      resolveLogin = resolve;
    });
  cancelBehavior = () => Promise.resolve(false);
  const { container, unmount } = await mount(GoogleSignInButton, {
    onComplete: async () => {
      completed++;
    },
  });
  try {
    await act(async () =>
      fireEvent.click(
        container.querySelector("[data-testid='onboarding-google-sign-in']"),
      ),
    );
    const cancel = [...container.querySelectorAll("button")].find(
      (b) => b.textContent === "Cancel sign-in",
    );
    await act(async () => fireEvent.click(cancel));
    assert.match(container.textContent, /Finishing sign-in in Buzz/);
    assert.doesNotMatch(container.textContent, /Cancel sign-in/);
    await act(async () => resolveLogin(baseStatus({ state: "active" })));
    assert.equal(completed, 1);
    assert.equal(
      calls.filter((c) => c.command === "login_with_google").length,
      1,
    );
    assert.equal(calls.filter((c) => c.command === "logout").length, 0);
  } finally {
    await unmount();
    loginBehavior = cancelBehavior = undefined;
  }
});

test("navigation away cancels its attempt and cannot invoke stale completion", async () => {
  status = baseStatus();
  let resolveLogin;
  let completed = 0;
  loginBehavior = () =>
    new Promise((resolve) => {
      resolveLogin = resolve;
    });
  const { container, unmount } = await mount(GoogleSignInButton, {
    onComplete: async () => {
      completed++;
    },
  });
  await act(async () =>
    fireEvent.click(
      container.querySelector("[data-testid='onboarding-google-sign-in']"),
    ),
  );
  const id = calls.find((c) => c.command === "login_with_google").args
    .attemptId;
  await unmount();
  assert.equal(
    calls.find((c) => c.command === "cancel_google_login").args.attemptId,
    id,
  );
  // Native registration can lag behind cleanup. The listener must still cancel it.
  const count = calls.filter((c) => c.command === "cancel_google_login").length;
  await act(async () => progress(id, "waiting"));
  assert.equal(
    calls.filter((c) => c.command === "cancel_google_login").length,
    count + 1,
  );
  await act(async () => resolveLogin(baseStatus({ state: "active" })));
  assert.equal(completed, 0);
  loginBehavior = undefined;
});

function dialogButton(label) {
  const dialog = document.querySelector('[role="alertdialog"]');
  assert.ok(dialog, "confirmation dialog is open");
  return [...dialog.querySelectorAll("button")].find(
    (b) => b.textContent.trim() === label,
  );
}

test("signing out, signing out other devices and disconnecting agents each confirm first", async () => {
  status = baseStatus({ state: "active", principal: "ab".repeat(32) });
  const { container, unmount } = await mount();
  await settle(() => container.querySelector("ul[aria-labelledby]") !== null);
  // Sign out last: the mocked logout never settles (the real one reloads).
  const cases = [
    [
      "account-sign-out-others",
      "Sign out other devices?",
      "Sign out other devices",
      "revoke_other_sessions",
    ],
    [
      "account-disconnect-agents",
      "Disconnect all agents?",
      "Disconnect agents",
      "revoke_all_bot_tokens",
    ],
    ["account-sign-out", "Sign out of Buzz?", "Sign out", "logout"],
  ];
  for (const [testId, title, confirmLabel, command] of cases) {
    const open = container.querySelector(`[data-testid='${testId}']`);
    await act(async () => fireEvent.click(open));
    const dialog = document.querySelector('[role="alertdialog"]');
    assert.ok(dialog, `${testId} opens a confirmation`);
    assert.ok(dialog.textContent.includes(title));
    assert.equal(
      calls.some((c) => c.command === command),
      false,
      `${testId} does nothing before confirmation`,
    );
    await act(async () => fireEvent.click(dialogButton("Cancel")));
    assert.equal(document.querySelector('[role="alertdialog"]'), null);
    assert.equal(
      calls.some((c) => c.command === command),
      false,
    );

    await act(async () => fireEvent.click(open));
    await act(async () => fireEvent.click(dialogButton(confirmLabel)));
    await settle(() => calls.some((c) => c.command === command));
    assert.equal(
      calls.filter((c) => c.command === command).length,
      1,
      `${testId} runs once after confirmation`,
    );
  }
  await unmount();
});

test("the sign-out confirmation tells the user how to get back in", async () => {
  status = baseStatus({
    state: "active",
    principal: "ab".repeat(32),
    keyBackup: true,
    keyBackupSupported: true,
  });
  const { container, unmount } = await mount();
  await act(async () =>
    fireEvent.click(
      container.querySelector("[data-testid='account-sign-out']"),
    ),
  );
  const dialog = document.querySelector('[role="alertdialog"]');
  assert.match(
    dialog.textContent,
    /You'll be signed out on this computer\. Sign in with Google to get back to your conversations\. Nothing is deleted\./,
  );
  assert.doesNotMatch(dialog.textContent, /agent/i, "no agent claims");
  await act(async () => fireEvent.click(dialogButton("Cancel")));
  await unmount();
});

const agentA = "a1".repeat(32);
const agentB = "b2".repeat(32);

async function openSignOutDialog() {
  status = baseStatus({
    state: "active",
    principal: "ab".repeat(32),
    keyBackup: true,
    keyBackupSupported: true,
  });
  const mounted = await mount();
  await act(async () =>
    fireEvent.click(
      mounted.container.querySelector("[data-testid='account-sign-out']"),
    ),
  );
  await settle(() =>
    calls.some((c) => c.command === "list_managed_agent_runtimes"),
  );
  await act(() => new Promise((resolve) => setTimeout(resolve, 10)));
  return mounted;
}

function account(pubkey, overrides = {}) {
  return {
    pubkey,
    origin: "http://127.0.0.1:3000",
    state: "signed_in",
    active: false,
    displayName: null,
    buzzName: null,
    avatarUrl: null,
    email: null,
    lastUsedAt: null,
    ...overrides,
  };
}

function dialogText() {
  return document.querySelector('[role="alertdialog"]').textContent;
}

test("the account card names the active account and its sign-out", async () => {
  accounts = [
    account("cd".repeat(32), {
      active: true,
      buzzName: "Jiho",
      displayName: "Jiho Google",
      email: "jiho@example.com",
    }),
    account("ef".repeat(32), { displayName: "Work" }),
  ];
  const { container, unmount } = await openSignOutDialog();
  assert.equal(
    container.querySelector("[data-testid='account-name']").textContent,
    "Jiho",
  );
  assert.match(container.textContent, /jiho@example\.com/);
  assert.match(dialogText(), /Sign out of Jiho\?/);
  // Another account here takes over: the dialog names it instead of
  // sending the user back to Google.
  assert.match(
    dialogText(),
    /signed out on this computer and switched to Work\. Nothing is deleted\./,
  );
  await act(async () => fireEvent.click(dialogButton("Cancel")));
  accounts = [];
  await unmount();
});

test("the dialog says how many of this account's agents stop; none, no line", async () => {
  runtimes = [];
  let { unmount } = await openSignOutDialog();
  assert.equal(
    document.querySelector("[data-testid='account-sign-out-agents']"),
    null,
  );
  assert.equal(
    document.querySelector("[role='alertdialog'] [role='checkbox']"),
    null,
    "no stop-agents choice",
  );
  await act(async () => fireEvent.click(dialogButton("Cancel")));
  await unmount();

  runtimes = [
    { pubkey: agentA, relayUrl: "wss://a.test", lifecycle: "ready" },
    { pubkey: agentA, relayUrl: "wss://b.test", lifecycle: "listening" },
    { pubkey: agentB, relayUrl: "wss://a.test", lifecycle: "starting" },
    { pubkey: "c3".repeat(32), relayUrl: "wss://a.test", lifecycle: "stopped" },
  ];
  ({ unmount } = await openSignOutDialog());
  await settle(
    () =>
      document.querySelector("[data-testid='account-sign-out-agents']") !==
      null,
  );
  assert.equal(
    document.querySelector("[data-testid='account-sign-out-agents']")
      .textContent,
    "2 agents of this account will stop.",
  );
  await act(async () => fireEvent.click(dialogButton("Cancel")));
  runtimes = [];
  await unmount();
});

test("another account's running agents are not counted", async () => {
  runtimes = [
    { pubkey: agentA, relayUrl: "wss://a.test", lifecycle: "ready" },
    { pubkey: agentB, relayUrl: "wss://a.test", lifecycle: "ready" },
  ];
  foreignAgents = new Set([agentB]);
  const { unmount } = await openSignOutDialog();
  await settle(
    () =>
      document.querySelector("[data-testid='account-sign-out-agents']") !==
      null,
  );
  assert.equal(
    document.querySelector("[data-testid='account-sign-out-agents']")
      .textContent,
    "1 agent of this account will stop.",
  );
  await act(async () => fireEvent.click(dialogButton("Cancel")));
  foreignAgents = new Set();
  runtimes = [];
  await unmount();
});

test("sign-out leaves stopping agents to the backend and reloads once", async (t) => {
  const reload = t.mock.method(tokenAuthApi.identityReload, "reload", () => {});
  runtimes = [{ pubkey: agentA, relayUrl: "wss://a.test", lifecycle: "ready" }];
  logoutBehavior = () => Promise.resolve(baseStatus({ deviceSignedOut: true }));
  const { unmount } = await openSignOutDialog();
  await act(async () => fireEvent.click(dialogButton("Sign out")));
  await settle(() => reload.mock.callCount() > 0);
  assert.deepEqual(calls.find((c) => c.command === "logout").args, {});
  assert.equal(
    calls.some((c) => c.command === "stop_managed_agent_runtime"),
    false,
    "the UI stops nothing itself",
  );
  assert.equal(reload.mock.callCount(), 1);
  assert.equal(window.sessionStorage.getItem("buzz:switching-to"), null);
  logoutBehavior = undefined;
  runtimes = [];
  await unmount();
});

test("signing out of the active account goes straight to the next one", async (t) => {
  const reload = t.mock.method(tokenAuthApi.identityReload, "reload", () => {});
  const next = "ef".repeat(32);
  accounts = [
    account("cd".repeat(32), { active: true, displayName: "Jiho" }),
    account(next, { displayName: "Work" }),
  ];
  logoutBehavior = () =>
    Promise.resolve(
      baseStatus({ state: "active", keyBackup: true, switchedTo: next }),
    );
  const { unmount } = await openSignOutDialog();
  await act(async () => fireEvent.click(dialogButton("Sign out")));
  await settle(() => reload.mock.callCount() > 0);
  assert.equal(reload.mock.callCount(), 1);
  assert.equal(
    window.sessionStorage.getItem("buzz:switching-to"),
    "Switching to Work…",
  );
  accountSwitch.takeAccountSwitchAtBoot();
  assert.equal(accountSwitch.accountSwitchCaption(), "Switching to Work…");
  accountSwitch.resetAccountSwitchForTests();
  logoutBehavior = undefined;
  accounts = [];
  await unmount();
});

test("a failed sign-out keeps the user here and says why", async (t) => {
  const reload = t.mock.method(tokenAuthApi.identityReload, "reload", () => {});
  logoutBehavior = () => Promise.reject("Could not reach the server.");
  const { unmount } = await openSignOutDialog();
  await act(async () => fireEvent.click(dialogButton("Sign out")));
  await settle(() => calls.some((c) => c.command === "logout"));
  await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
  assert.equal(reload.mock.callCount(), 0);
  logoutBehavior = undefined;
  await unmount();
});

test("onStatusChange reports each settled status to the Profile panel", async () => {
  status = baseStatus({ state: "active", keyBackup: true });
  const seen = [];
  const { unmount } = await mount(AccountSettingsCard, {
    onStatusChange: (next) => seen.push(next),
  });
  assert.ok(seen.length >= 1);
  assert.ok(seen.every((next) => next !== null));
  assert.equal(seen.at(-1).keyBackup, true);
  await unmount();
});
