import assert from "node:assert/strict";
import test from "node:test";

// Local data deletion for Google accounts: no key backup gate, no private key
// fetched or shown, plain "sign in with Google again" copy, and the typed
// phrase still guards the wipe. The E2E-only key seam keeps the backup gate.

const calls = [];
const tauriMock = {
  invoke(command) {
    calls.push(command);
    if (command === "get_nsec")
      return Promise.resolve(`nsec1${"q".repeat(58)}`);
    if (command === "sign_out") return new Promise(() => {});
    return Promise.reject(new Error(`unmocked Tauri command: ${command}`));
  },
  transformCallback: () => Math.random(),
  unregisterCallback() {},
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_INTERNALS__ = tauriMock;

const { fireEvent } = await import("@testing-library/react");
const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { SignOutSection, SIGNOUT_CONFIRM_PHRASE } = await import(
  "./SignOutSection.tsx"
);

async function mount(props) {
  calls.length = 0;
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () =>
    root.render(React.createElement(SignOutSection, props)),
  );
  return {
    container,
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

async function openDialog(container) {
  await act(async () =>
    fireEvent.click(
      container.querySelector("[data-testid='signout-open-dialog']"),
    ),
  );
  await act(() => new Promise((resolve) => setTimeout(resolve, 10)));
  const dialog = document.querySelector('[role="alertdialog"]');
  assert.ok(dialog);
  return dialog;
}

test("Google accounts delete local data without any key backup step", async () => {
  const { container, unmount } = await mount();
  assert.match(container.textContent, /sign in with Google again/);
  assert.doesNotMatch(container.textContent, /\bkey\b|backup/i);
  const dialog = await openDialog(container);
  assert.match(dialog.textContent, /sign in with Google again to get back in/);
  assert.doesNotMatch(dialog.textContent, /\bkey\b|backup|nsec/i);
  assert.equal(
    dialog.querySelector("[data-testid='signout-backup-confirm']"),
    null,
  );
  assert.equal(
    calls.includes("get_nsec"),
    false,
    "the private key is never read",
  );

  const confirm = document.querySelector("[data-testid='signout-confirm']");
  assert.equal(
    confirm.disabled,
    true,
    "the typed phrase still guards the wipe",
  );
  const input = document.querySelector(
    "[data-testid='signout-confirm-phrase']",
  );
  await act(async () =>
    fireEvent.change(input, { target: { value: SIGNOUT_CONFIRM_PHRASE } }),
  );
  assert.equal(confirm.disabled, false);
  await act(async () => fireEvent.click(confirm));
  assert.equal(calls.filter((c) => c === "sign_out").length, 1);
  await unmount();
  document.body.innerHTML = "";
});

test("the E2E key seam keeps the backup gate", async () => {
  const { container, unmount } = await mount({ showKeyBackupGate: true });
  await openDialog(container);
  await act(() => new Promise((resolve) => setTimeout(resolve, 10)));
  assert.ok(document.querySelector("[data-testid='signout-backup-confirm']"));
  assert.equal(calls.includes("get_nsec"), true);
  const input = document.querySelector(
    "[data-testid='signout-confirm-phrase']",
  );
  await act(async () =>
    fireEvent.change(input, { target: { value: SIGNOUT_CONFIRM_PHRASE } }),
  );
  assert.equal(
    document.querySelector("[data-testid='signout-confirm']").disabled,
    true,
    "the phrase alone is not enough without the backup confirmation",
  );
  await unmount();
  document.body.innerHTML = "";
});
