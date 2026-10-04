import assert from "node:assert/strict";
import test from "node:test";

// Settings › Account binds the real card to the real Tauri command names: a
// relay without token sign-in shows nothing, a signed-out community offers
// Google sign-in, and a signed-in one lists devices with one labelled
// sign-out button per other device (no duplicate stop for this device).

const calls = [];
let status;
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
        return Promise.resolve(null);
      case "logout":
      case "login_with_google":
        // Never settles: the real flow waits on the browser, then reloads.
        return new Promise(() => {});
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
const { AccountSettingsCard } = await import("./AccountSettingsCard.tsx");

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

async function mount() {
  calls.length = 0;
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(React.createElement(AccountSettingsCard));
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
  await settle(() => button.textContent.includes("Waiting"));
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
  const buttons = [...list.querySelectorAll("button")];
  assert.equal(buttons.length, 1, "the current device has no sign-out button");
  assert.equal(buttons[0].getAttribute("aria-label"), "Sign out Work PC");
  await act(async () => {
    fireEvent.click(buttons[0]);
  });
  await settle(() => calls.some((c) => c.command === "revoke_device"));
  assert.deepEqual(calls.find((c) => c.command === "revoke_device").args, {
    deviceId: devices[1].id,
  });
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
