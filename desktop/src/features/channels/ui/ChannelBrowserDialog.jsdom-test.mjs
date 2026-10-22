import assert from "node:assert/strict";
import test from "node:test";

// Find groups lists the groups you are in. Public groups you are not in only
// show up once the search has 2 or more characters, and come from the
// server search (`GET /api/groups/search`), so the dialog never lists every
// public group.

const searchCalls = [];
const tauriMock = {
  invoke(command, args) {
    if (command === "groups_request") {
      searchCalls.push(args.path);
      const q = new URLSearchParams(args.path.split("?")[1]).get("q") ?? "";
      const groups = [
        {
          id: "g-design",
          name: "design",
          description: null,
          handle: null,
          channelType: "stream",
          memberCount: 3,
          isMember: true,
        },
        {
          id: "g-deploys",
          name: "deploys",
          description: null,
          handle: null,
          channelType: "stream",
          memberCount: 3,
          isMember: false,
        },
      ].filter((item) => item.name.includes(q));
      return Promise.resolve({ groups });
    }
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
  queryClient.setQueryData(["identity"], { pubkey: "a".repeat(64) });
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
    // The list filters on a deferred query and the server search is
    // debounced 250 ms; let both settle.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 300));
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50));
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
    assert.equal(searchCalls.length, 0, "no server search below 2");

    await view.type("de");
    assert.ok(view.find("browse-channel-design"));
    assert.ok(view.find("browse-channel-deploys"), "public match after 2");
    assert.ok(view.find("channel-browser-section-public"));
    assert.ok(searchCalls[0].startsWith("/api/groups/search?q=de&limit=20"));
    // A joined group from search is not listed twice.
    assert.equal(
      document.body.querySelectorAll("[data-testid='browse-channel-design']")
        .length,
      1,
    );
    // Private groups you are not in never appear.
    assert.equal(view.find("browse-channel-desk"), null);
    assert.equal(view.find("channel-browser-search-hint"), null);
  } finally {
    await view.unmount();
  }
});
