import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// Friends are only added explicitly. The old per-device "seed friends from
// DMs" step re-added people removed on another PC and turned spam DM senders
// into friends, so People must never write the friend list on its own.

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");

test("People never adds DM partners as friends automatically", () => {
  const friends = read("./usePeopleFriends.ts");
  const list = read("./ui/PeopleSidebarList.tsx");
  for (const source of [friends, list]) {
    assert.doesNotMatch(source, /SeedFriends|seeded-from-dms|dmPeople/);
  }
  // The friends hook only edits the list through explicit add/remove.
  assert.doesNotMatch(friends, /editContactList|setContactList/);
});
