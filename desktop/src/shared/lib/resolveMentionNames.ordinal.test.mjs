import assert from "node:assert/strict";
import test from "node:test";

import { resolveMentionProps } from "./resolveMentionNames.ts";

const A = "a".repeat(64);
const B = "b".repeat(64);

test("received same-name ordinals render as the shared name and bind nobody wrongly", () => {
  const { mentionNames, mentionPubkeysByName } = resolveMentionProps(
    [
      ["p", A],
      ["p", B],
    ],
    { [A]: { displayName: "Scout" }, [B]: { displayName: "Scout" } },
    "@Scout @Scout (2) hi",
  );
  assert.ok(mentionNames?.includes("Scout"));
  // Two tagged people share "Scout": neither occurrence binds a profile.
  assert.equal(mentionPubkeysByName?.scout, undefined);
});

test("a unique name still binds its profile", () => {
  const { mentionPubkeysByName } = resolveMentionProps(
    [["p", A]],
    { [A]: { displayName: "Scout" } },
    "@Scout hi",
  );
  assert.equal(mentionPubkeysByName?.scout, A);
});
