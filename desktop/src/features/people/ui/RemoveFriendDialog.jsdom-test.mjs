import assert from "node:assert/strict";
import test from "node:test";

// Removing a friend asks first and only removes after "Remove".

const ALICE = "a".repeat(64);
const tauriMock = {
  invoke() {
    // Theme and window chrome calls have nothing to check here.
    return Promise.resolve(null);
  },
  transformCallback() {
    return Math.random();
  },
  unregisterCallback() {},
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_INTERNALS__ = tauriMock;
window.matchMedia ??= () => ({
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
const { RemoveFriendDialog, friendAddedMessage, friendRemovedMessage } =
  await import("./RemoveFriendDialog.tsx");
const { ThemeProvider } = await import(
  "../../../shared/theme/ThemeProvider.tsx"
);

async function mount({ removeFriend } = {}) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  const removed = [];
  let closed = 0;
  await act(async () => {
    root.render(
      React.createElement(
        ThemeProvider,
        null,
        React.createElement(RemoveFriendDialog, {
          onClose: () => {
            closed += 1;
          },
          onRemoveFriend:
            removeFriend ??
            (async (pubkey) => {
              removed.push(pubkey);
            }),
          target: { pubkey: ALICE, name: "Alice" },
        }),
      ),
    );
  });
  const find = (testId) =>
    document.body.querySelector(`[data-testid='${testId}']`);
  return {
    closed: () => closed,
    find,
    removed,
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

test("asks before removing and names the person", async () => {
  const view = await mount();
  const dialog = view.find("remove-friend-dialog");
  assert.ok(dialog, "confirm dialog missing");
  assert.match(dialog.textContent, /Remove Alice from friends\?/);
  assert.deepEqual(view.removed, []);
  await view.unmount();
});

test("Remove removes the friend and closes the dialog", async () => {
  const view = await mount();
  await act(async () => fireEvent.click(view.find("remove-friend-confirm")));
  assert.deepEqual(view.removed, [ALICE]);
  assert.equal(view.closed(), 1);
  await view.unmount();
});

test("a failed removal still closes the dialog", async () => {
  const view = await mount({
    removeFriend: async () => {
      throw new Error("offline");
    },
  });
  await act(async () => fireEvent.click(view.find("remove-friend-confirm")));
  assert.equal(view.closed(), 1);
  await view.unmount();
});

test("toast text for add and remove names the person", () => {
  assert.equal(friendAddedMessage("Alice"), "Alice is now your friend");
  assert.equal(
    friendRemovedMessage("Alice"),
    "Alice was removed from your friends",
  );
});
