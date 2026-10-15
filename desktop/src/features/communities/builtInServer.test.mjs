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
  ensureAllowedServer,
  getBuiltInServer,
  isAllowedServer,
  isSameServer,
  loadBuiltInServer,
  pinStoredCommunitiesToBuiltInServer,
  pinnedActiveCommunityId,
  resolveBuiltInServer,
  resolveStartupPin,
  retryBuiltInServer,
  serverKey,
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

test("servers compare by host + port over the same kind of scheme", () => {
  assert.equal(serverKey(BUILT_IN), "https://jihobuzz.duckdns.org:8443");
  for (const same of [
    "wss://jihobuzz.duckdns.org:8443/",
    "wss://JIHOBUZZ.duckdns.org:8443",
    "https://jihobuzz.duckdns.org:8443/invite/abc",
    "jihobuzz.duckdns.org:8443",
  ]) {
    assert.ok(isSameServer(same, BUILT_IN), same);
  }
  for (const other of [
    // Unencrypted forms never match the (encrypted) built-in server.
    "ws://jihobuzz.duckdns.org:8443",
    "http://jihobuzz.duckdns.org:8443",
    "wss://jihobuzz.duckdns.org",
    "wss://jihobuzz.duckdns.org:9443",
    "wss://evil.example:8443",
    "",
    null,
    "ftp://jihobuzz.duckdns.org:8443",
  ]) {
    assert.equal(isSameServer(other, BUILT_IN), false, String(other));
  }
  // A dev build's unencrypted override is reached over ws/http alike.
  assert.ok(isSameServer("http://localhost:3000", "ws://localhost:3000"));
  assert.equal(
    isSameServer("wss://localhost:3000", "ws://localhost:3000"),
    false,
  );
});

test("no built-in value for E2E mock builds or an unusable address", () => {
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
  const ours = community("ours", "https://jihobuzz.duckdns.org:8443/");
  const pin = resolveStartupPin([other, ours], "other", BUILT_IN);
  assert.deepEqual(pin, {
    communities: [other, ours],
    activeId: "ours",
    created: null,
  });
  assert.equal(resolveStartupPin([other, ours], "ours", BUILT_IN), null);
  assert.equal(resolveStartupPin([other, ours], "other", null), null);
  assert.equal(resolveStartupPin([], null, BUILT_IN), null);
  // An unencrypted entry for our host is not ours: a new entry is added.
  const insecure = community("insecure", "ws://jihobuzz.duckdns.org:8443");
  const added = resolveStartupPin([insecure], "insecure", BUILT_IN);
  assert.ok(added?.created);
  assert.equal(added.created.relayUrl, BUILT_IN);
});

test("startup adds a fresh, signed-out built-in community when only others are stored", () => {
  const other = community("other", "wss://old.example", {
    pubkey: "ab",
    token: "secret",
    reposDir: "/repos",
  });
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
    addedAt: "2026-10-15T00:00:00.000Z",
  });
  assert.equal("pubkey" in pin.created, false, "no foreign account copied");
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

test("loading resolves once and pins storage", async () => {
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
  assert.equal(await loadBuiltInServer(read, "e2e"), null);
});

test("a failed read is an error with retry, never the multi-server mode", async () => {
  for (const failing of [
    async () => {
      throw new Error("no bridge");
    },
    async () => "   ",
    async () => null,
  ]) {
    setBuiltInServerForTests(undefined);
    assert.equal(await loadBuiltInServer(failing, "production"), null);
    assert.equal(getBuiltInServer(), null);
    // Fails closed: nothing may be sent anywhere while in the error state.
    assert.throws(
      () => ensureAllowedServer(BUILT_IN),
      /This invite is for a different service\./,
    );
  }

  // Retry reads again and recovers.
  let reads = 0;
  assert.equal(
    await retryBuiltInServer(async () => {
      reads += 1;
      return BUILT_IN;
    }, "production"),
    BUILT_IN,
  );
  assert.equal(reads, 1);
  assert.equal(getBuiltInServer(), BUILT_IN);
  assert.doesNotThrow(() => ensureAllowedServer(`${BUILT_IN}/`));
});

test("only the built-in server is allowed for webview requests", () => {
  setBuiltInServerForTests(undefined);
  assert.throws(() => ensureAllowedServer(BUILT_IN), /different service/);
  setBuiltInServerForTests(BUILT_IN);
  assert.doesNotThrow(() => ensureAllowedServer(BUILT_IN));
  assert.doesNotThrow(() =>
    ensureAllowedServer("https://jihobuzz.duckdns.org:8443"),
  );
  for (const refused of [
    "wss://evil.example",
    "https://evil.example:8443",
    "http://jihobuzz.duckdns.org:8443",
    "not a url",
  ]) {
    assert.throws(
      () => ensureAllowedServer(refused),
      /different service/,
      refused,
    );
  }
  // E2E mock builds (no built-in server) allow every host.
  setBuiltInServerForTests(null);
  assert.doesNotThrow(() => ensureAllowedServer("wss://evil.example"));
});
