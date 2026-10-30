import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";

import { setDeviceSnapshotDeps } from "../../../shared/api/deviceSnapshot.ts";
import {
  getUserLabelsVersion,
  hydrateUserLabels,
  hydrateWatchedUserLabels,
  readCachedUserLabels,
  removeUserLabelCacheForRelay,
  resetUserLabelStorageCache,
  resetUserLabelsForTests,
  resolveUserLabelPlaceholderData,
  userLabelCacheKey,
  writeCachedUserLabels,
} from "./userLabelStorage.ts";

const ME = "aa".repeat(32);
const OTHER = "bb".repeat(32);
const RELAY = "wss://relay.example";
const scope = { pubkey: ME, relayUrl: RELAY };

function installLocalStorage() {
  const values = new Map();
  globalThis.window = {
    localStorage: {
      getItem: (key) => values.get(key) ?? null,
      setItem: (key, value) => values.set(key, value),
      removeItem: (key) => values.delete(key),
      key: (index) => [...values.keys()][index] ?? null,
      get length() {
        return values.size;
      },
    },
  };
  globalThis.localStorage = globalThis.window.localStorage;
  return values;
}

/** A device store keyed like the app's: one blob per account, server, kind. */
function installStore({ writable = true, sole = true, readGate } = {}) {
  const blobs = new Map();
  const key = (kind, s) => `${kind}|${s.relayUrl}|${s.pubkey.toLowerCase()}`;
  const writes = [];
  const signedInReads = [];
  setDeviceSnapshotDeps({
    read: async (kind, s) => {
      if (readGate) await readGate;
      return blobs.get(key(kind, s)) ?? null;
    },
    readSignedIn: async (kind, s) => {
      signedInReads.push(s);
      return blobs.get(key(kind, s)) ?? null;
    },
    write: async (kind, s, blob) => {
      writes.push({ kind, scope: s, blob });
      if (!writable) return false;
      blobs.set(key(kind, s), blob);
      return true;
    },
    soleAccount: async () => sole,
  });
  return { blobs, key, signedInReads, writes };
}

const legacyAlice = JSON.stringify({
  version: 1,
  profiles: {
    abcdef: { displayName: "Old Alice", name: null, nip05Handle: null },
  },
});

const alice = {
  displayName: "Alice",
  name: "alice",
  avatarUrl: "https://relay.example/media/a.png",
  nip05Handle: null,
  ownerPubkey: null,
};

beforeEach(() => {
  installLocalStorage();
  resetUserLabelsForTests();
});

afterEach(() => {
  resetUserLabelsForTests();
  setDeviceSnapshotDeps();
});

test("labels kept on the device are read at launch, avatars included", async () => {
  const store = installStore();
  store.blobs.set(
    store.key("profiles", scope),
    JSON.stringify({
      version: 1,
      profiles: { ABCDEF: { ...alice, updatedAt: 100 } },
    }),
  );
  const before = getUserLabelsVersion();
  assert.equal(readCachedUserLabels(scope, ["abcdef"]), undefined);

  await hydrateUserLabels(scope);

  assert.ok(getUserLabelsVersion() > before, "re-render signal");
  assert.deepEqual(readCachedUserLabels(scope, ["ABCDEF", "missing"]), {
    profiles: {
      abcdef: {
        displayName: "Alice",
        name: "alice",
        avatarUrl: "https://relay.example/media/a.png",
        nip05Handle: null,
        ownerPubkey: null,
      },
    },
    missing: [],
  });
});

test("another account on the same server never sees them", async () => {
  const store = installStore();
  store.blobs.set(
    store.key("profiles", scope),
    JSON.stringify({ version: 1, profiles: { abcdef: alice } }),
  );
  const other = { pubkey: OTHER, relayUrl: RELAY };
  await hydrateUserLabels(scope);
  await hydrateUserLabels(other);
  assert.ok(readCachedUserLabels(scope, ["abcdef"]));
  assert.equal(readCachedUserLabels(other, ["abcdef"]), undefined);
  assert.equal(readCachedUserLabels(null, ["abcdef"]), undefined);
});

test("labels from earlier builds move to the sole account once, then the old key goes", async () => {
  const store = installStore();
  window.localStorage.setItem(
    "kuzz-user-labels.v1:wss://relay.example",
    legacyAlice,
  );
  writeCachedUserLabels(scope, { ghijkl: { ...alice, displayName: "Bob" } });

  await hydrateUserLabels(scope);

  assert.equal(
    readCachedUserLabels(scope, ["abcdef"]).profiles.abcdef.displayName,
    "Old Alice",
  );
  assert.equal(store.writes.length, 1);
  assert.match(store.writes[0].blob, /Old Alice/);
  assert.match(store.writes[0].blob, /Bob/, "the merged set is written");
  assert.equal(window.localStorage.getItem(userLabelCacheKey(RELAY)), null);
});

test("with other accounts signed in on that server, the old key goes unmoved", async () => {
  const store = installStore({ sole: false });
  window.localStorage.setItem(userLabelCacheKey(RELAY), legacyAlice);

  await hydrateUserLabels(scope);

  assert.equal(readCachedUserLabels(scope, ["abcdef"]), undefined);
  assert.equal(store.writes.length, 0);
  assert.equal(window.localStorage.getItem(userLabelCacheKey(RELAY)), null);
});

test("a save made before the launch read waits for it and keeps the stored labels", async () => {
  let release;
  const store = installStore({
    readGate: new Promise((resolve) => {
      release = resolve;
    }),
  });
  store.blobs.set(
    store.key("profiles", scope),
    JSON.stringify({ version: 1, profiles: { abcdef: alice } }),
  );
  void hydrateUserLabels(scope);
  writeCachedUserLabels(scope, { ghijkl: { ...alice, displayName: "Bob" } });

  await new Promise((resolve) => setTimeout(resolve, 1_700));
  assert.equal(store.writes.length, 0, "held until the read finishes");
  release();
  await new Promise((resolve) => setTimeout(resolve, 20));

  assert.equal(store.writes.length, 1);
  const saved = JSON.parse(store.writes[0].blob);
  assert.deepEqual(Object.keys(saved.profiles).sort(), ["abcdef", "ghijkl"]);
});

test("switching accounts writes a waiting save instead of dropping it", async () => {
  const store = installStore();
  await hydrateUserLabels(scope);
  writeCachedUserLabels(scope, { abcdef: alice });

  resetUserLabelStorageCache();
  await new Promise((resolve) => setTimeout(resolve, 20));

  assert.equal(store.writes.length, 1);
  assert.match(store.writes[0].blob, /Alice/);
});

test("a watched account's labels are read without writing or taking over", async () => {
  const store = installStore();
  const watched = { pubkey: OTHER, relayUrl: RELAY };
  store.blobs.set(
    store.key("profiles", watched),
    JSON.stringify({ version: 1, profiles: { abcdef: alice } }),
  );
  window.localStorage.setItem(userLabelCacheKey(RELAY), legacyAlice);

  await hydrateWatchedUserLabels(watched);
  await hydrateWatchedUserLabels(watched);

  assert.equal(store.signedInReads.length, 1, "once per run");
  assert.equal(
    readCachedUserLabels(watched, ["abcdef"]).profiles.abcdef.displayName,
    "Alice",
  );
  assert.equal(readCachedUserLabels(scope, ["abcdef"]), undefined);
  assert.equal(store.writes.length, 0);
  assert.ok(window.localStorage.getItem(userLabelCacheKey(RELAY)));
});

test("the old key stays when the store did not take the labels", async () => {
  installStore({ writable: false });
  window.localStorage.setItem(
    userLabelCacheKey(RELAY),
    JSON.stringify({
      version: 1,
      profiles: {
        abcdef: { displayName: "Old", name: null, nip05Handle: null },
      },
    }),
  );
  await hydrateUserLabels(scope);
  assert.ok(window.localStorage.getItem(userLabelCacheKey(RELAY)));
  assert.ok(readCachedUserLabels(scope, ["abcdef"]));
});

test("a relay answer before the launch read is not overwritten by older kept labels", async () => {
  const store = installStore();
  store.blobs.set(
    store.key("profiles", scope),
    JSON.stringify({
      version: 1,
      profiles: { abcdef: { ...alice, displayName: "Stale", updatedAt: 1 } },
    }),
  );
  writeCachedUserLabels(scope, { abcdef: { ...alice, displayName: "Fresh" } });
  await hydrateUserLabels(scope);
  assert.equal(
    readCachedUserLabels(scope, ["abcdef"]).profiles.abcdef.displayName,
    "Fresh",
  );
});

test("keeps previous full profiles ahead of kept placeholders", () => {
  installStore();
  writeCachedUserLabels(scope, { abcdef: { ...alice, displayName: "Kept" } });
  const previous = {
    profiles: { abcdef: { ...alice, displayName: "Fresh", ownerPubkey: "o" } },
    missing: [],
  };
  assert.equal(
    resolveUserLabelPlaceholderData(previous, scope, ["abcdef"]),
    previous,
  );
  assert.equal(
    resolveUserLabelPlaceholderData(undefined, scope, ["abcdef"]).profiles
      .abcdef.displayName,
    "Kept",
  );
});

test("writes stay bounded to the 500 most recent and are saved for the account", async () => {
  const store = installStore();
  writeCachedUserLabels(
    scope,
    Object.fromEntries(
      Array.from({ length: 505 }, (_, index) => [
        `pubkey-${index}`,
        { ...alice, displayName: `Person ${index}` },
      ]),
    ),
  );
  const known = Array.from({ length: 505 }, (_, index) => `pubkey-${index}`);
  assert.equal(
    Object.keys(readCachedUserLabels(scope, known).profiles).length,
    500,
  );
  await new Promise((resolve) => setTimeout(resolve, 1_700));
  assert.equal(store.writes.length, 1);
  assert.equal(store.writes[0].kind, "profiles");
  assert.equal(store.writes[0].scope.pubkey, ME);
  const saved = JSON.parse(store.writes[0].blob);
  assert.equal(Object.keys(saved.profiles).length, 500);
});

test("removes labels the relay cleared or reports missing", () => {
  installStore();
  writeCachedUserLabels(scope, { abcdef: alice, ghijkl: alice });
  writeCachedUserLabels(scope, {
    abcdef: {
      displayName: null,
      name: null,
      avatarUrl: null,
      nip05Handle: null,
      ownerPubkey: null,
    },
  });
  writeCachedUserLabels(scope, {}, ["GHIJKL"]);
  assert.equal(readCachedUserLabels(scope, ["abcdef", "ghijkl"]), undefined);
});

test("removing a community forgets its labels and the old key only", () => {
  installStore();
  const second = { pubkey: ME, relayUrl: "wss://two.example" };
  writeCachedUserLabels(scope, { abcdef: alice });
  writeCachedUserLabels(second, { abcdef: alice });
  window.localStorage.setItem(userLabelCacheKey(RELAY), "{}");
  window.localStorage.setItem(userLabelCacheKey(second.relayUrl), "{}");

  removeUserLabelCacheForRelay(RELAY);

  assert.equal(readCachedUserLabels(scope, ["abcdef"]), undefined);
  assert.ok(readCachedUserLabels(second, ["abcdef"]));
  assert.equal(window.localStorage.getItem(userLabelCacheKey(RELAY)), null);
  assert.equal(
    window.localStorage.getItem(userLabelCacheKey(second.relayUrl)),
    "{}",
  );
});

test("ignores malformed kept payloads", async () => {
  const store = installStore();
  store.blobs.set(
    store.key("profiles", scope),
    JSON.stringify({ version: 1, profiles: { abc: { displayName: 42 } } }),
  );
  await hydrateUserLabels(scope);
  assert.equal(readCachedUserLabels(scope, ["abc"]), undefined);
});
