import assert from "node:assert/strict";
import test from "node:test";

// "Add friend" finds one person by exact Kuzz ID or email, or says no one.

const ALICE = "a".repeat(64);
const lookups = [];
// When set, the next lookup waits until the test resolves it.
let holdNextLookup = null;
const tauriMock = {
  invoke(command, args) {
    if (command === "lookup_person") {
      lookups.push(args.query);
      if (holdNextLookup) {
        const hold = holdNextLookup;
        holdNextLookup = null;
        return hold.promise;
      }
      return Promise.resolve(
        args.query === "alice"
          ? {
              pubkey: ALICE,
              displayName: "Alice",
              avatarUrl: null,
              username: "alice",
            }
          : null,
      );
    }
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
// The dialog follows the app theme, which reads the system color scheme.
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
const { AddFriendDialog, NO_ONE_FOUND } = await import("./AddFriendDialog.tsx");
const { ThemeProvider } = await import(
  "../../../shared/theme/ThemeProvider.tsx"
);

async function mount(props = {}) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  const added = [];
  const messaged = [];
  const closed = [];
  await act(async () => {
    root.render(
      React.createElement(
        ThemeProvider,
        null,
        React.createElement(AddFriendDialog, {
          currentPubkey: "f".repeat(64),
          isBlocked: () => false,
          isFriend: () => false,
          onAddFriend: async (pubkey) => {
            added.push(pubkey);
          },
          onMessage: (pubkey) => messaged.push(pubkey),
          onOpenChange: (open) => closed.push(open),
          open: true,
          ...props,
        }),
      ),
    );
  });
  const find = (testId) =>
    document.body.querySelector(`[data-testid='${testId}']`);
  const search = async (text) => {
    await act(async () =>
      fireEvent.change(find("add-friend-query"), { target: { value: text } }),
    );
    await act(async () => fireEvent.click(find("add-friend-search")));
  };
  return {
    added,
    closed,
    find,
    messaged,
    search,
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

test("an exact ID finds the person and Add makes them a friend", async () => {
  lookups.length = 0;
  const view = await mount();
  await view.search("  alice ");
  assert.deepEqual(lookups, ["alice"]);
  assert.match(view.find("add-friend-result").textContent, /Alice/);
  await act(async () => fireEvent.click(view.find("add-friend-add")));
  assert.deepEqual(view.added, [ALICE]);
  await view.unmount();
});

test("an unknown ID or email says no one was found", async () => {
  const view = await mount();
  await view.search("nobody@example.com");
  assert.equal(view.find("add-friend-not-found").textContent, NO_ONE_FOUND);
  assert.equal(view.find("add-friend-add"), null);
  await view.unmount();
});

test("a blocked person can't be added from here", async () => {
  const view = await mount({ isBlocked: (pubkey) => pubkey === ALICE });
  await view.search("alice");
  assert.match(view.find("add-friend-result").textContent, /Blocked/);
  assert.equal(view.find("add-friend-add"), null);
  await view.unmount();
});

test("after Add the result says Added and offers Message", async () => {
  const view = await mount();
  await view.search("alice");
  await act(async () => fireEvent.click(view.find("add-friend-add")));
  assert.deepEqual(view.added, [ALICE]);
  assert.match(view.find("add-friend-added").textContent, /Added/);
  assert.equal(view.find("add-friend-add"), null);
  // The dialog stays open so Message is reachable.
  assert.deepEqual(view.closed, []);
  await act(async () => fireEvent.click(view.find("add-friend-message")));
  assert.deepEqual(view.messaged, [ALICE]);
  await view.unmount();
});

test("a slow lookup for an old query never shows under the new query", async () => {
  let release;
  holdNextLookup = {
    promise: new Promise((resolve) => {
      release = resolve;
    }),
  };
  const view = await mount();
  await view.search("alice");
  // Change the query while "alice" is still being looked up.
  await act(async () =>
    fireEvent.change(view.find("add-friend-query"), {
      target: { value: "bob" },
    }),
  );
  await act(async () =>
    release({
      pubkey: ALICE,
      displayName: "Alice",
      avatarUrl: null,
      username: "alice",
    }),
  );
  assert.equal(view.find("add-friend-result"), null);
  assert.equal(view.find("add-friend-add"), null);
  await view.unmount();
});
