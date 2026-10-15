import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";

// The single-server gate: which server is built in, startup pinning of the
// stored communities, and the host guard every invite/link/account path uses.

class MemoryStorage {
  #items = new Map();
  getItem(key) {
    return this.#items.has(key) ? this.#items.get(key) : null;
  }
  setItem(key, value) {
    this.#items.set(key, String(value));
  }
  removeItem(key) {
    this.#items.delete(key);
  }
  clear() {
    this.#items.clear();
  }
}
globalThis.localStorage = new MemoryStorage();
globalThis.window ??= globalThis;
globalThis.window.localStorage = globalThis.localStorage;
const tauriCalls = [];
globalThis.__TAURI_INTERNALS__ = {
  invoke(command, args) {
    tauriCalls.push({ command, args });
    return Promise.resolve(null);
  },
};

const {
  communityLinkDecision,
  getBuiltInServer,
  isAllowedServer,
  isSameServer,
  loadBuiltInServer,
  pinStoredCommunitiesToBuiltInServer,
  pinnedActiveCommunityId,
  resolveBuiltInServer,
  resolveStartupPin,
  serverHostKey,
  setBuiltInServerForTests,
} = await import("./builtInServer.ts");
const { loadActiveCommunityId, loadCommunities, saveCommunities } =
  await import("./communityStorage.ts");

const BUILT_IN = "wss://jihobuzz.duckdns.org:8443";

function community(id, relayUrl, extra = {}) {
  return {
    id,
    name: id,
    relayUrl,
    addedAt: "2026-01-01T00:00:00.000Z",
    ...extra,
  };
}

beforeEach(() => {
  globalThis.localStorage.clear();
  setBuiltInServerForTests(undefined);
});

test("servers compare by host + port across ws/wss/http/https", () => {
  assert.equal(serverHostKey(BUILT_IN), "jihobuzz.duckdns.org:8443");
  for (const same of [
    "wss://jihobuzz.duckdns.org:8443/",
    "ws://JIHOBUZZ.duckdns.org:8443",
    "https://jihobuzz.duckdns.org:8443/invite/abc",
    "http://jihobuzz.duckdns.org:8443",
    "jihobuzz.duckdns.org:8443",
  ]) {
    assert.ok(isSameServer(same, BUILT_IN), same);
  }
  for (const other of [
    "wss://jihobuzz.duckdns.org",
    "wss://jihobuzz.duckdns.org:9443",
    "wss://evil.example:8443",
    "",
    null,
    "ftp://jihobuzz.duckdns.org:8443",
  ]) {
    assert.equal(isSameServer(other, BUILT_IN), false, String(other));
  }
});

test("the gate is off in E2E mock builds and without a value", () => {
  assert.equal(resolveBuiltInServer(BUILT_IN, "e2e"), null);
  assert.equal(resolveBuiltInServer(null, "production"), null);
  assert.equal(resolveBuiltInServer("   ", "production"), null);
  assert.equal(resolveBuiltInServer(` ${BUILT_IN} `, "production"), BUILT_IN);
  assert.equal(resolveBuiltInServer(BUILT_IN, "development"), BUILT_IN);
});

test("without a built-in server every host is allowed", () => {
  assert.equal(isAllowedServer("wss://evil.example", null), true);
  assert.equal(isAllowedServer("wss://evil.example", BUILT_IN), false);
  assert.equal(isAllowedServer(`${BUILT_IN}/`, BUILT_IN), true);
});

test("community links: other hosts are rejected, add-community is dropped", () => {
  assert.equal(
    communityLinkDecision("join", "wss://x.example", null),
    "follow",
  );
  assert.equal(
    communityLinkDecision("join", "wss://x.example", BUILT_IN),
    "reject",
  );
  assert.equal(
    communityLinkDecision("connect", "wss://x.example", BUILT_IN),
    "reject",
  );
  assert.equal(
    communityLinkDecision("add-community", "wss://x.example", BUILT_IN),
    "reject",
  );
  assert.equal(communityLinkDecision("join", BUILT_IN, BUILT_IN), "follow");
  assert.equal(communityLinkDecision("connect", BUILT_IN, BUILT_IN), "follow");
  assert.equal(
    communityLinkDecision("add-community", BUILT_IN, BUILT_IN),
    "ignore",
  );
});

test("the active community stays on the built-in server", () => {
  const other = community("other", "wss://old.example");
  const ours = community("ours", `${BUILT_IN}/`);
  assert.equal(
    pinnedActiveCommunityId([other, ours], "other", BUILT_IN),
    "ours",
  );
  assert.equal(pinnedActiveCommunityId([other, ours], null, BUILT_IN), "ours");
  assert.equal(
    pinnedActiveCommunityId([other, ours], "ours", BUILT_IN),
    "ours",
  );
  assert.equal(pinnedActiveCommunityId([other, ours], "other", null), "other");
  assert.equal(pinnedActiveCommunityId([other], "other", BUILT_IN), "other");
});

test("startup selects the stored built-in community", () => {
  const other = community("other", "wss://old.example");
  const ours = community("ours", "ws://jihobuzz.duckdns.org:8443");
  const pin = resolveStartupPin([other, ours], "other", BUILT_IN);
  assert.deepEqual(pin, {
    communities: [other, ours],
    activeId: "ours",
    created: null,
  });
  assert.equal(resolveStartupPin([other, ours], "ours", BUILT_IN), null);
  assert.equal(resolveStartupPin([other, ours], "other", null), null);
  assert.equal(resolveStartupPin([], null, BUILT_IN), null);
});

test("startup adds the built-in community when only others are stored", () => {
  const other = community("other", "wss://old.example", { pubkey: "ab" });
  const pin = resolveStartupPin(
    [other],
    "other",
    BUILT_IN,
    () => "new-id",
    () => new Date("2026-10-15T00:00:00.000Z"),
  );
  assert.ok(pin);
  assert.equal(pin.activeId, "new-id");
  assert.deepEqual(pin.communities[0], other, "others stay stored");
  assert.deepEqual(pin.created, {
    id: "new-id",
    name: "jihobuzz",
    relayUrl: BUILT_IN,
    pubkey: "ab",
    addedAt: "2026-10-15T00:00:00.000Z",
  });
});

test("pinning persists the choice and never deletes stored communities", () => {
  const other = community("other", "wss://old.example");
  const ours = community("ours", BUILT_IN);
  saveCommunities([other, ours]);
  globalThis.localStorage.setItem("buzz-active-community-id", "other");

  pinStoredCommunitiesToBuiltInServer(BUILT_IN);

  assert.equal(loadActiveCommunityId(), "ours");
  assert.deepEqual(
    loadCommunities().map((entry) => entry.id),
    ["other", "ours"],
  );
});

test("loading resolves once, pins storage, and falls back to null on failure", async () => {
  const other = community("other", "wss://old.example");
  saveCommunities([other]);
  globalThis.localStorage.setItem("buzz-active-community-id", "other");
  let reads = 0;
  const read = async () => {
    reads += 1;
    return BUILT_IN;
  };

  assert.equal(getBuiltInServer(), null, "nothing until loaded");
  assert.equal(await loadBuiltInServer(read, "production"), BUILT_IN);
  assert.equal(await loadBuiltInServer(read, "production"), BUILT_IN);
  assert.equal(reads, 1);
  assert.equal(getBuiltInServer(), BUILT_IN);
  const stored = loadCommunities();
  assert.equal(stored.length, 2);
  assert.equal(loadActiveCommunityId(), stored[1].id);
  assert.equal(stored[1].relayUrl, BUILT_IN);
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.ok(
    tauriCalls.some(
      (call) =>
        call.command === "readd_community_relay" &&
        call.args?.relayUrl === BUILT_IN,
    ),
    "the added built-in community re-admits local agents",
  );

  setBuiltInServerForTests(undefined);
  assert.equal(
    await loadBuiltInServer(async () => {
      throw new Error("no bridge");
    }, "production"),
    null,
  );
  assert.equal(getBuiltInServer(), null);

  setBuiltInServerForTests(undefined);
  assert.equal(await loadBuiltInServer(read, "e2e"), null);
});
