import assert from "node:assert/strict";
import test from "node:test";

// A blocked author's message folds in channels and group DMs, but shows as
// it is in a 1:1 DM (the server already left out what came during the block).

const ME = "a".repeat(64);
const BOB = "b".repeat(64);
const CAROL = "c".repeat(64);

const tauriMock = {
  invoke() {
    return Promise.resolve(null);
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
const { QueryClient, QueryClientProvider } = await import(
  "@tanstack/react-query"
);
const { BlockedFoldScope, useFoldsBlockedAuthor } = await import(
  "./blockedFoldScope.tsx"
);
const { BlockedMessageNotice } = await import("./BlockedMessageNotice.tsx");
const { BlockedPeopleProvider, blockedPeopleQueryKey } = await import(
  "../../people/BlockedPeopleContext.tsx"
);

// Mirrors the timeline and thread rows: fold to the notice, or show the text.
function Row({ pubkey }) {
  const folds = useFoldsBlockedAuthor(pubkey);
  return folds
    ? React.createElement(BlockedMessageNotice, { onShow() {} })
    : React.createElement("p", { "data-testid": "message" }, "hello");
}

async function render(channel, { scoped = true } = {}) {
  const queryClient = new QueryClient();
  queryClient.setQueryData(["identity"], { pubkey: ME });
  queryClient.setQueryData(blockedPeopleQueryKey(ME), [
    { pubkey: BOB, createdAt: null },
  ]);
  const row = React.createElement(Row, { pubkey: BOB });
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(
      React.createElement(
        QueryClientProvider,
        { client: queryClient },
        React.createElement(
          BlockedPeopleProvider,
          null,
          scoped
            ? React.createElement(BlockedFoldScope, { channel }, row)
            : row,
        ),
      ),
    );
  });
  const folded = Boolean(
    container.querySelector("[data-testid='blocked-message-notice']"),
  );
  await act(async () => root.unmount());
  container.remove();
  return folded;
}

test("1:1 DM shows a blocked person's message", async () => {
  assert.equal(
    await render({ channelType: "dm", participantPubkeys: [ME, BOB] }),
    false,
  );
});

test("group DM folds a blocked author", async () => {
  assert.equal(
    await render({ channelType: "dm", participantPubkeys: [ME, BOB, CAROL] }),
    true,
  );
});

test("channel folds a blocked author", async () => {
  assert.equal(
    await render({ channelType: "stream", participantPubkeys: [] }),
    true,
  );
});

test("rows outside a conversation scope fold, as before", async () => {
  assert.equal(await render(null, { scoped: false }), true);
});
