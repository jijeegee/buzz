import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

// Startup pinning through the real provider: once the built-in server loads,
// the provider adopts the pinned storage before its children render with the
// old choice, keeps every stored community, and refuses to switch away.

const tauriMock = {
  invoke: () => Promise.resolve(null),
  transformCallback: () => Math.random(),
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_INTERNALS__ = tauriMock;

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { CommunitiesProvider, useCommunities } = await import(
  "./useCommunities.tsx"
);
const { loadBuiltInServer, setBuiltInServerForTests } = await import(
  "./builtInServer.ts"
);

const BUILT_IN = "wss://jihobuzz.duckdns.org:8443";
const entry = (id, relayUrl) => ({
  id,
  name: id,
  relayUrl,
  addedAt: "2026-01-01T00:00:00.000Z",
});

let root = null;
let container = null;

afterEach(async () => {
  if (root) await act(async () => root.unmount());
  container?.remove();
  root = null;
  container = null;
  setBuiltInServerForTests(undefined);
  localStorage.clear();
});

async function mount() {
  const seen = [];
  let api = null;
  function Probe() {
    api = useCommunities();
    seen.push(api.activeCommunity?.id ?? null);
    return null;
  }
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root.render(
      React.createElement(
        CommunitiesProvider,
        null,
        React.createElement(Probe),
      ),
    );
  });
  return { seen, current: () => api };
}

test("loading the built-in server selects its stored community", async () => {
  localStorage.setItem(
    "kuzz-communities",
    JSON.stringify([
      entry("other", "wss://old.example"),
      entry("ours", BUILT_IN),
    ]),
  );
  localStorage.setItem("kuzz-active-community-id", "other");
  setBuiltInServerForTests(undefined);
  const { seen, current } = await mount();
  assert.equal(seen.at(-1), "other", "upstream choice while loading");

  seen.length = 0;
  await act(async () => {
    await loadBuiltInServer(async () => BUILT_IN, "production");
  });
  assert.deepEqual([...new Set(seen)], ["ours"]);
  assert.equal(localStorage.getItem("kuzz-active-community-id"), "ours");
  assert.equal(current().communities.length, 2, "nothing deleted");

  await act(async () => current().switchCommunity("other"));
  assert.equal(current().activeCommunity?.id, "ours");
});

test("only other servers stored: the built-in one is added and selected", async () => {
  localStorage.setItem(
    "kuzz-communities",
    JSON.stringify([entry("other", "wss://old.example")]),
  );
  localStorage.setItem("kuzz-active-community-id", "other");
  const { current } = await mount();
  await act(async () => {
    await loadBuiltInServer(async () => BUILT_IN, "production");
  });
  const api = current();
  assert.equal(api.communities.length, 2);
  assert.equal(api.communities[0].id, "other");
  assert.equal(api.activeCommunity?.relayUrl, BUILT_IN);
});
