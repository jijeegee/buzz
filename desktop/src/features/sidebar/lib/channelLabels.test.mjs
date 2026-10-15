import assert from "node:assert/strict";
import test from "node:test";

import { resolveChannelDisplayLabel } from "./channelLabels.ts";

const ME = "a".repeat(64);
const key = (char) => char.repeat(64);

function groupDm(pubkeys) {
  return {
    channelType: "dm",
    name: "Group DM",
    participantPubkeys: [ME, ...pubkeys],
    participants: [],
  };
}

function profilesFor(names) {
  return Object.fromEntries(
    Object.entries(names).map(([pubkey, displayName]) => [
      pubkey,
      { displayName },
    ]),
  );
}

test("a single unnamed participant shows the plain unknown label", () => {
  const label = resolveChannelDisplayLabel(
    groupDm([key("b"), key("c")]),
    ME,
    profilesFor({ [key("b")]: "Alice" }),
  );
  assert.equal(label, "Alice, Unknown user");
});

test("several unnamed participants collapse into one counted entry", () => {
  const label = resolveChannelDisplayLabel(
    groupDm([key("b"), key("c"), key("d")]),
    ME,
    profilesFor({ [key("b")]: "Alice" }),
  );
  assert.equal(label, "Alice, Unknown user (2)");
  assert.doesNotMatch(label, /[0-9a-f]{8}|npub/);
});

test("only unnamed participants still count every person", () => {
  const label = resolveChannelDisplayLabel(
    groupDm([key("b"), key("c"), key("d")]),
    ME,
    {},
  );
  assert.equal(label, "Unknown user (3)");
});

test("+N more counts hidden people, including unnamed ones", () => {
  const label = resolveChannelDisplayLabel(
    groupDm([key("b"), key("c"), key("d"), key("e"), key("f"), key("1")]),
    ME,
    profilesFor({
      [key("b")]: "Alice",
      [key("c")]: "Bob",
      [key("d")]: "Carol",
      [key("e")]: "Dave",
    }),
  );
  // 6 other people: 3 visible names, Dave plus two unnamed people hidden.
  assert.equal(label, "Alice, Bob, Carol, +3 more");
});

test("participants are deduped by pubkey, not by label", () => {
  const label = resolveChannelDisplayLabel(
    groupDm([key("b"), key("c"), key("b")]),
    ME,
    profilesFor({ [key("b")]: "Alice", [key("c")]: "Alice" }),
  );
  assert.equal(label, "Alice, Alice");
});

test("named channels keep their name", () => {
  const channel = { ...groupDm([key("b")]), name: "Launch crew" };
  assert.equal(resolveChannelDisplayLabel(channel, ME, {}), "Launch crew");
});
