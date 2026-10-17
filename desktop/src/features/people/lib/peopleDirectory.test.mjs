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
  const { favorites, friends, inChannels } = buildPeopleDirectory({
    ...base,
    friendIds: new Set(["cc", "aa", "me", "bot2"]),
  });
  assert.deepEqual(
    friends.map((u) => u.pubkey),
    ["aa", "cc"],
  );
  assert.deepEqual(favorites, []);
  assert.deepEqual(inChannels, []);
});

test("the search box filters only friends; others come only from my channels", () => {
  // The whole directory is known locally, yet a non-friend never shows
  // unless the server returned them as sharing a channel with me.
  const { friends, inChannels } = buildPeopleDirectory({
    ...base,
    filter: "a",
    friendIds: new Set(["cc"]),
  });
  assert.deepEqual(
    friends.map((u) => u.pubkey),
    ["cc"],
  );
  assert.deepEqual(inChannels, []);
  const shared = buildPeopleDirectory({
    ...base,
    filter: "a",
    friendIds: new Set(["cc"]),
    sharedPeople: [
      user("DD", "dana"),
      user("cc", "carol"),
      user("aa", "Alice"),
      user("me", "Me"),
      user("bot3", "A Bot", { isAgent: true }),
      user("dd", "dana"),
    ],
  });
  // carol is already a friend; me, agents and duplicates never show.
  assert.deepEqual(
    shared.inChannels.map((u) => u.pubkey),
    ["aa", "dd"],
  );
  const idle = buildPeopleDirectory({
    ...base,
    sharedPeople: [user("dd", "dana")],
  });
  assert.deepEqual(idle.inChannels, []);
});

test("blocked people never show in any section", () => {
  const isBlocked = (pubkey) => pubkey === "aa" || pubkey === "dd";
  const { favorites, friends, inChannels } = buildPeopleDirectory({
    ...base,
    favoriteIds: new Set(["aa"]),
    filter: "a",
    friendIds: new Set(["aa", "cc"]),
    isBlocked,
    sharedPeople: [user("dd", "dana"), user("ee", "eva")],
  });
  assert.deepEqual(favorites, []);
  assert.deepEqual(
    friends.map((u) => u.pubkey),
    ["cc"],
  );
  assert.deepEqual(
    inChannels.map((u) => u.pubkey),
    ["ee"],
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
  assert.deepEqual(filtered.inChannels, []);
});
