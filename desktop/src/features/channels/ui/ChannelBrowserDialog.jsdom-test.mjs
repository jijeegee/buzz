import assert from "node:assert/strict";
import test from "node:test";

// Find groups lists the groups you are in. Public groups you are not in only
// show up once the search has 2 or more characters, so the dialog never
// lists every public group.

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
window.requestAnimationFrame ??= (callback) => setTimeout(callback, 0);
window.cancelAnimationFrame ??= (id) => clearTimeout(id);
globalThis.ResizeObserver ??= class {
  observe() {}
  disconnect() {}
};
if (!document.fonts) {
  Object.defineProperty(document, "fonts", {
    configurable: true,
    value: { ready: Promise.resolve() },
  });
}
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
const { QueryClient, QueryClientProvider } = await import(
  "@tanstack/react-query"
);
const { ChannelBrowserDialog, FIND_GROUPS_MIN_QUERY_LENGTH } = await import(
  "./ChannelBrowserDialog.tsx"
);
const { ThemeProvider } = await import(
  "../../../shared/theme/ThemeProvider.tsx"
);

function group(id, name, { isMember, visibility = "open" }) {
  return {
    archivedAt: null,
    channelType: "stream",
    description: "",
    id,
    isMember,
    lastMessageAt: null,
    memberCount: 3,
    name,
    participantPubkeys: [],
    participants: [],
    visibility,
  };
}

const CHANNELS = [
  group("g-design", "design", { isMember: true }),
  group("g-deploys", "deploys", { isMember: false }),
  group("g-secret", "desk", { isMember: false, visibility: "private" }),
];

async function mount() {
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
        React.createElement(
          ThemeProvider,
          null,
          React.createElement(ChannelBrowserDialog, {
            channels: CHANNELS,
            onJoinChannel: async () => {},
            onOpenChange: () => {},
            onSelectChannel: () => {},
            open: true,
          }),
        ),
      ),
    );
  });
  const find = (testId) =>
    document.body.querySelector(`[data-testid='${testId}']`);
  const type = async (text) => {
    await act(async () => {
      fireEvent.change(find("channel-browser-search"), {
        target: { value: text },
      });
    });
    // The list filters on a deferred query; let it settle.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
  };
  return {
    find,
    type,
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
      queryClient.clear();
    },
  };
}

test("Find groups hides public groups until 2 characters are typed", async () => {
  assert.equal(FIND_GROUPS_MIN_QUERY_LENGTH, 2);
  const view = await mount();
  try {
    assert.match(document.body.textContent, /Find groups/);
    assert.match(document.body.textContent, /My groups/);
    assert.ok(view.find("browse-channel-design"), "joined group listed");
    assert.equal(view.find("browse-channel-deploys"), null);
    assert.ok(view.find("channel-browser-search-hint"));

    await view.type("d");
    assert.ok(view.find("browse-channel-design"));
    assert.equal(view.find("browse-channel-deploys"), null);

    await view.type("de");
    assert.ok(view.find("browse-channel-design"));
    assert.ok(view.find("browse-channel-deploys"), "public match after 2");
    assert.ok(view.find("channel-browser-section-public"));
    // Private groups you are not in never appear.
    assert.equal(view.find("browse-channel-desk"), null);
    assert.equal(view.find("channel-browser-search-hint"), null);
  } finally {
    await view.unmount();
  }
});
