import assert from "node:assert/strict";
import test from "node:test";
import {
  DEFAULT_COMMUNITY_THEME,
  appearanceModeOf,
  cacheAndApplyCommunityTheme,
  clearCommunityThemeOutbox,
  communityThemeApplyExpectation,
  communityThemeOutboxKey,
  communityThemePersistenceAction,
  communityThemeScopeFallback,
  communityThemeStorageKey,
  parseCommunityThemePreference,
  readCommunityThemeOutbox,
  readCommunityThemePreference,
  readDeviceAppearanceMode,
  sameCommunityThemePreference,
  withDeviceAppearanceMode,
  writeCommunityThemeOutbox,
  writeCommunityThemePreference,
  writeDeviceAppearanceMode,
} from "./communityThemePreference.ts";

function localStorageStub() {
  const data = new Map();
  return {
    getItem: (key) => data.get(key) ?? null,
    setItem: (key, value) => data.set(key, String(value)),
    removeItem: (key) => data.delete(key),
  };
}

test("parses only the versioned stable appearance contract", () => {
  const valid = {
    version: 1,
    theme: "houston",
    accent: "#a855f7",
    followSystem: false,
  };
  assert.deepEqual(parseCommunityThemePreference(valid), valid);
  assert.equal(parseCommunityThemePreference({ ...valid, version: 2 }), null);
  assert.equal(
    parseCommunityThemePreference({ ...valid, theme: "future-theme" }),
    null,
  );
  assert.equal(
    parseCommunityThemePreference({ ...valid, accent: "url(image)" }),
    null,
  );
  assert.equal(
    parseCommunityThemePreference({ ...valid, followSystem: "false" }),
    null,
  );
});

test("local preferences are isolated by pubkey and normalized relay", () => {
  globalThis.window = { localStorage: localStorageStub() };
  const aliceA = {
    ...DEFAULT_COMMUNITY_THEME,
    theme: "houston",
    followSystem: false,
  };
  const aliceB = { ...DEFAULT_COMMUNITY_THEME, theme: "catppuccin-latte" };
  const bobA = { ...DEFAULT_COMMUNITY_THEME, accent: "#ef4444" };
  assert.equal(
    writeCommunityThemePreference("alice", "WSS://A.EXAMPLE/", aliceA),
    true,
  );
  assert.equal(
    writeCommunityThemePreference("alice", "wss://b.example", aliceB),
    true,
  );
  assert.equal(
    writeCommunityThemePreference("bob", "wss://a.example", bobA),
    true,
  );
  assert.deepEqual(
    readCommunityThemePreference("alice", "wss://a.example"),
    aliceA,
  );
  assert.deepEqual(
    readCommunityThemePreference("alice", "wss://b.example/"),
    aliceB,
  );
  assert.deepEqual(
    readCommunityThemePreference("bob", "wss://a.example"),
    bobA,
  );
  assert.notEqual(
    communityThemeStorageKey("alice", "wss://a.example"),
    communityThemeStorageKey("alice", "wss://b.example"),
  );
});

test("dirty outbox survives restart and clears only its exact revision", () => {
  globalThis.window = { localStorage: localStorageStub() };
  const first = { ...DEFAULT_COMMUNITY_THEME, theme: "houston" };
  const second = { ...DEFAULT_COMMUNITY_THEME, accent: "#ef4444" };

  assert.equal(
    writeCommunityThemeOutbox("alice", "WSS://A.EXAMPLE/", first),
    true,
  );
  assert.deepEqual(readCommunityThemeOutbox("alice", "wss://a.example"), first);
  writeCommunityThemeOutbox("alice", "wss://a.example", second);
  clearCommunityThemeOutbox("alice", "wss://a.example", first);
  assert.deepEqual(
    readCommunityThemeOutbox("alice", "wss://a.example"),
    second,
  );
  clearCommunityThemeOutbox("alice", "wss://a.example", second);
  assert.equal(readCommunityThemeOutbox("alice", "wss://a.example"), null);
  assert.notEqual(
    communityThemeOutboxKey("alice", "wss://a.example"),
    communityThemeStorageKey("alice", "wss://a.example"),
  );
});

test("malformed local data returns null so switching can apply the safe default", () => {
  globalThis.window = { localStorage: localStorageStub() };
  const key = communityThemeStorageKey("alice", "wss://broken.example");
  window.localStorage.setItem(
    key,
    JSON.stringify({ version: 1, theme: "missing" }),
  );
  assert.equal(
    readCommunityThemePreference("alice", "wss://broken.example"),
    null,
  );
  window.localStorage.setItem(key, "{");
  assert.equal(
    readCommunityThemePreference("alice", "wss://broken.example"),
    null,
  );
});

test("remote preference still applies when its local cache write fails", () => {
  globalThis.window = {
    localStorage: {
      getItem: () => null,
      setItem: () => {
        throw new Error("quota exceeded");
      },
    },
  };
  let applied = null;
  cacheAndApplyCommunityTheme(
    "alice",
    "wss://a.example",
    DEFAULT_COMMUNITY_THEME,
    (preference) => {
      applied = preference;
    },
  );
  assert.deepEqual(applied, DEFAULT_COMMUNITY_THEME);
});

test("already-applied relay state leaves the next user edit publishable", () => {
  const applied = {
    ...DEFAULT_COMMUNITY_THEME,
    theme: "catppuccin-latte",
    followSystem: false,
  };

  assert.equal(communityThemeApplyExpectation(applied, applied), null);
  assert.deepEqual(
    communityThemeApplyExpectation(applied, DEFAULT_COMMUNITY_THEME),
    applied,
  );
});

test("no-op initialization remains programmatic", () => {
  const expectation = communityThemeApplyExpectation(
    DEFAULT_COMMUNITY_THEME,
    DEFAULT_COMMUNITY_THEME,
    true,
  );

  assert.equal(
    communityThemePersistenceAction(expectation, DEFAULT_COMMUNITY_THEME),
    "acknowledge",
  );
});

test("confirmed first-community migration isolates later empty scopes", () => {
  const inherited = {
    ...DEFAULT_COMMUNITY_THEME,
    theme: "dracula",
    followSystem: false,
  };

  assert.deepEqual(communityThemeScopeFallback(false, inherited), inherited);
  assert.deepEqual(
    communityThemeScopeFallback(true, inherited),
    DEFAULT_COMMUNITY_THEME,
  );
});

test("community switch defers stale outgoing appearance persistence", () => {
  const outgoing = {
    ...DEFAULT_COMMUNITY_THEME,
    theme: "houston",
    followSystem: false,
  };
  const incoming = {
    ...DEFAULT_COMMUNITY_THEME,
    theme: "catppuccin-latte",
  };

  assert.equal(communityThemePersistenceAction(incoming, outgoing), "defer");
  assert.equal(
    communityThemePersistenceAction(incoming, incoming),
    "acknowledge",
  );
  assert.equal(communityThemePersistenceAction(null, incoming), "persist");
});

test("device appearance mode is overlaid on a synced preference", () => {
  const synced = {
    ...DEFAULT_COMMUNITY_THEME,
    theme: "github-light",
    followSystem: false,
  };

  assert.deepEqual(withDeviceAppearanceMode(synced, "dark"), {
    ...synced,
    theme: "github-dark",
  });
  assert.deepEqual(withDeviceAppearanceMode(synced, "light"), synced);
  assert.deepEqual(withDeviceAppearanceMode(synced, "system"), {
    ...synced,
    followSystem: true,
  });
  assert.equal(appearanceModeOf(synced), "light");
  assert.equal(
    appearanceModeOf(withDeviceAppearanceMode(synced, "dark")),
    "dark",
  );
});

test("mode-only differences are not a different synced preference", () => {
  const light = {
    ...DEFAULT_COMMUNITY_THEME,
    theme: "github-light",
    followSystem: false,
  };
  const dark = { ...light, theme: "github-dark" };
  const system = { ...light, followSystem: true };

  assert.equal(sameCommunityThemePreference(light, dark), true);
  assert.equal(sameCommunityThemePreference(light, system), true);
  assert.equal(
    sameCommunityThemePreference(light, { ...light, theme: "dracula" }),
    false,
  );
  assert.equal(
    sameCommunityThemePreference(light, { ...light, accent: "#ef4444" }),
    false,
  );
});

test("device appearance mode is stored once per device", () => {
  globalThis.window = { localStorage: localStorageStub() };
  assert.equal(readDeviceAppearanceMode(), null);
  writeDeviceAppearanceMode("dark");
  assert.equal(readDeviceAppearanceMode(), "dark");
});
