import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";

const {
  configureService,
  currentServiceUrl,
  isServiceAddress,
  migrateServiceStorage,
  relayStorageScope,
  resetServiceForTests,
  SERVICE_MIGRATION_KEY,
} = await import("./serviceAddress.ts");
const { normalizeRelayUrl } = await import("./normalizeRelayUrl.ts");

class MemoryStorage {
  #items = new Map();
  get length() {
    return this.#items.size;
  }
  key(index) {
    return [...this.#items.keys()][index] ?? null;
  }
  getItem(key) {
    return this.#items.has(key) ? this.#items.get(key) : null;
  }
  setItem(key, value) {
    this.#items.set(key, String(value));
  }
  removeItem(key) {
    this.#items.delete(key);
  }
}

const OLD = "wss://jihobuzz.duckdns.org:8443";
const NEW = "wss://jihobuzz.duckdns.org";

beforeEach(() => {
  configureService({ scope: "kuzz", current: NEW, previous: [OLD] });
});

test("service addresses key storage by the scope", () => {
  for (const url of [
    OLD,
    `${OLD}/`,
    NEW,
    "https://jihobuzz.duckdns.org:8443",
    "https://JIHOBUZZ.duckdns.org",
    "wss://jihobuzz.duckdns.org:443",
  ]) {
    assert.equal(isServiceAddress(url), true, url);
    assert.equal(relayStorageScope(url, "fallback"), "kuzz", url);
    assert.equal(normalizeRelayUrl(url), "kuzz", url);
  }
  for (const url of [
    "wss://relay.example",
    "ws://jihobuzz.duckdns.org:8443",
    "wss://jihobuzz.duckdns.org:9443",
  ]) {
    assert.equal(isServiceAddress(url), false, url);
    assert.equal(relayStorageScope(url, "fallback"), "fallback", url);
  }
  assert.equal(
    normalizeRelayUrl("wss://Relay.Example/"),
    "wss://relay.example",
  );
});

test("without a built-in server nothing is scoped", () => {
  resetServiceForTests();
  assert.equal(isServiceAddress(OLD), false);
  assert.equal(normalizeRelayUrl(`${OLD}/`), OLD);
});

test("media on an earlier address loads from the current one", () => {
  assert.equal(
    currentServiceUrl("https://jihobuzz.duckdns.org:8443/media/ab.jpg?x=1"),
    "https://jihobuzz.duckdns.org/media/ab.jpg?x=1",
  );
  assert.equal(
    currentServiceUrl("https://relay.example/media/ab.jpg"),
    "https://relay.example/media/ab.jpg",
  );
});

test("stored settings move to the scope once and the old keys stay", () => {
  const storage = new MemoryStorage();
  const pubkey = "ab".repeat(32);
  storage.setItem(`kuzz-channel-snapshot.v1:${OLD}:${pubkey}`, "snap");
  storage.setItem(
    `kuzz-channel-sections.v1:${pubkey}:${encodeURIComponent(OLD)}`,
    "sections",
  );
  storage.setItem(`kuzz-drafts.v2:${OLD}:${pubkey}`, "drafts");
  storage.setItem(
    `project-sidebar.${encodeURIComponent("https://jihobuzz.duckdns.org:8443")}.${pubkey}`,
    "membership",
  );
  storage.setItem("kuzz-thread-activity.v1:wss://relay.example:ab", "other");
  // Written by this build already: not overwritten.
  storage.setItem(`kuzz-observed-unread.v1:kuzz:${pubkey}`, "new");
  storage.setItem(`kuzz-observed-unread.v1:${OLD}:${pubkey}`, "old");

  assert.equal(migrateServiceStorage(storage), 4);
  assert.equal(
    storage.getItem(`kuzz-channel-snapshot.v1:kuzz:${pubkey}`),
    "snap",
  );
  assert.equal(
    storage.getItem(`kuzz-channel-sections.v1:${pubkey}:kuzz`),
    "sections",
  );
  assert.equal(storage.getItem(`kuzz-drafts.v2:kuzz:${pubkey}`), "drafts");
  assert.equal(storage.getItem(`project-sidebar.kuzz.${pubkey}`), "membership");
  assert.equal(
    storage.getItem(`kuzz-observed-unread.v1:kuzz:${pubkey}`),
    "new",
  );
  assert.equal(storage.getItem(`kuzz-drafts.v2:${OLD}:${pubkey}`), "drafts");

  // Once per address: a setting cleared after the move stays cleared.
  storage.removeItem(`kuzz-drafts.v2:kuzz:${pubkey}`);
  assert.equal(migrateServiceStorage(storage), 0);
  assert.equal(storage.getItem(`kuzz-drafts.v2:kuzz:${pubkey}`), null);
  assert.ok(storage.getItem(SERVICE_MIGRATION_KEY));
});

test("keys on the current address do not swallow a longer old address", () => {
  const storage = new MemoryStorage();
  storage.setItem(`kuzz-drafts.v2:${OLD}:ab`, "old");
  storage.setItem(`kuzz-drafts.v2:${NEW}:cd`, "current");
  migrateServiceStorage(storage);
  assert.equal(storage.getItem("kuzz-drafts.v2:kuzz:ab"), "old");
  assert.equal(storage.getItem("kuzz-drafts.v2:kuzz:cd"), "current");
  assert.equal(storage.getItem("kuzz-drafts.v2:kuzz:8443:ab"), null);
});

test("an address moved earlier is not matched inside a longer one later", () => {
  const storage = new MemoryStorage();
  const pubkey = `3${"a".repeat(63)}`;
  storage.setItem(`kuzz-drafts.v2:${OLD}:${pubkey}`, "old");
  storage.setItem(
    `kuzz-channel-sections.v1:${pubkey}:${encodeURIComponent(OLD)}`,
    "sections",
  );
  // A build on the old address moved it already; this one only adds NEW.
  storage.setItem(
    SERVICE_MIGRATION_KEY,
    JSON.stringify(["https://jihobuzz.duckdns.org:8443"]),
  );
  assert.equal(migrateServiceStorage(storage), 0);
  assert.equal(storage.getItem(`kuzz-drafts.v2:kuzz:8443:${pubkey}`), null);

  // A digit-first pubkey after the address still moves.
  storage.removeItem(SERVICE_MIGRATION_KEY);
  assert.equal(migrateServiceStorage(storage), 2);
  assert.equal(storage.getItem(`kuzz-drafts.v2:kuzz:${pubkey}`), "old");
});
