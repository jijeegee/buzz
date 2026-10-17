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
  filter: "",
  myAgentPubkeys: new Set(),
  users: USERS,
};

test("Everyone is A-Z without me or my agents, keeping others' agents", () => {
  const { everyone, favorites } = buildPeopleDirectory(base);
  assert.deepEqual(
    everyone.map((u) => u.pubkey),
    ["aa", "bb", "cc", "bot2"],
  );
  assert.deepEqual(favorites, []);
});

test("managed agents and archived people are left out", () => {
  const { everyone } = buildPeopleDirectory({
    ...base,
    isArchived: (pubkey) => pubkey === "cc",
    myAgentPubkeys: new Set(["bot2"]),
  });
  assert.deepEqual(
    everyone.map((u) => u.pubkey),
    ["aa", "bb"],
  );
});

test("favorites sit on top, also from a batch profile, and obey the filter", () => {
  const favoriteIds = new Set(["bb", "zz", "me"]);
  const favoriteProfiles = {
    zz: {
      displayName: "Zed",
      avatarUrl: null,
      nip05Handle: null,
      ownerPubkey: null,
    },
  };
  const all = buildPeopleDirectory({ ...base, favoriteIds, favoriteProfiles });
  assert.deepEqual(
    all.favorites.map((u) => u.pubkey),
    ["bb", "zz"],
  );
  const filtered = buildPeopleDirectory({
    ...base,
    favoriteIds,
    favoriteProfiles,
    filter: " ZE ",
  });
  assert.deepEqual(
    filtered.favorites.map((u) => u.pubkey),
    ["zz"],
  );
  assert.deepEqual(filtered.everyone, []);
});

test("the name filter is case-insensitive", () => {
  const { everyone } = buildPeopleDirectory({ ...base, filter: "BO" });
  assert.deepEqual(
    everyone.map((u) => u.pubkey),
    ["bb", "bot2"],
  );
});
