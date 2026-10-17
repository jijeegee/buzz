import assert from "node:assert/strict";
import test from "node:test";

import { buildPeopleDirectory } from "./peopleDirectory.ts";

const user = (pubkey, displayName, extra = {}) => ({
  pubkey,
  displayName,
  avatarUrl: null,
  nip05Handle: null,
  ownerPubkey: null,
  isAgent: false,
  ...extra,
});

const USERS = [
  user("cc", "carol"),
  user("me", "Me"),
  user("aa", "Alice"),
  user("bot1", "My Bot", { isAgent: true, ownerPubkey: "me" }),
  user("bot2", "Their Bot", { isAgent: true, ownerPubkey: "aa" }),
  user("bb", "bob"),
];

const base = {
  currentPubkey: "me",
  favoriteIds: new Set(),
  friendIds: new Set(),
  filter: "",
  users: USERS,
};

test("Friends are only people I added, A-Z, never me or any agent", () => {
  const { favorites, friends, others } = buildPeopleDirectory({
    ...base,
    friendIds: new Set(["cc", "aa", "me", "bot2"]),
  });
  assert.deepEqual(
    friends.map((u) => u.pubkey),
    ["aa", "cc"],
  );
  assert.deepEqual(favorites, []);
  assert.deepEqual(others, []);
});

test("typing a name also finds people who are not friends yet", () => {
  const { friends, others } = buildPeopleDirectory({
    ...base,
    filter: "b",
    friendIds: new Set(["bb"]),
  });
  assert.deepEqual(
    friends.map((u) => u.pubkey),
    ["bb"],
  );
  // bob is already a friend; agents named "Bot" never show.
  assert.deepEqual(others, []);
  const all = buildPeopleDirectory({ ...base, filter: "a" });
  assert.deepEqual(
    all.others.map((u) => u.pubkey),
    ["aa", "cc"],
  );
});

test("archived people are left out", () => {
  const { friends } = buildPeopleDirectory({
    ...base,
    friendIds: new Set(["aa", "cc"]),
    isArchived: (pubkey) => pubkey === "cc",
  });
  assert.deepEqual(
    friends.map((u) => u.pubkey),
    ["aa"],
  );
});

test("favorites and friends also come from batch profiles and obey the filter", () => {
  const favoriteIds = new Set(["bb", "zz", "me"]);
  const extraProfiles = {
    zz: {
      displayName: "Zed",
      avatarUrl: null,
      nip05Handle: null,
      ownerPubkey: null,
    },
    yy: {
      displayName: "Yoda",
      avatarUrl: null,
      nip05Handle: null,
      ownerPubkey: null,
      isAgent: true,
    },
  };
  const all = buildPeopleDirectory({
    ...base,
    extraProfiles,
    favoriteIds,
    friendIds: new Set(["zz", "yy"]),
  });
  assert.deepEqual(
    all.favorites.map((u) => u.pubkey),
    ["bb", "zz"],
  );
  assert.deepEqual(
    all.friends.map((u) => u.pubkey),
    ["zz"],
  );
  const filtered = buildPeopleDirectory({
    ...base,
    extraProfiles,
    favoriteIds,
    filter: " ZE ",
  });
  assert.deepEqual(
    filtered.favorites.map((u) => u.pubkey),
    ["zz"],
  );
  assert.deepEqual(filtered.others, []);
});
