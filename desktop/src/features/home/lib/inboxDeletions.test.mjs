import assert from "node:assert/strict";
import test from "node:test";

import {
  collectInboxReferencedEventIds,
  getDeletedEventIds,
  withoutDeletedFeedItems,
} from "./inboxDeletions.ts";

const ROOT = "a".repeat(64);
const REPLY = "b".repeat(64);
const OTHER = "c".repeat(64);
const MAIN = "d".repeat(64);

const item = (id, tags = []) => ({ id, kind: 9, tags });

function feed(mentions, activity = []) {
  return {
    feed: { activity, agentActivity: [], mentions, needsAction: [] },
    meta: {},
  };
}

test("collects each row's message plus its thread root and parent", () => {
  const ids = collectInboxReferencedEventIds(
    feed(
      [
        item(REPLY, [
          ["e", ROOT, "", "root"],
          ["e", ROOT, "", "reply"],
        ]),
      ],
      [item(MAIN)],
    ).feed,
  );
  assert.deepEqual(ids, [ROOT, REPLY, MAIN].sort());
});

test("only deletion kinds mark targets deleted", () => {
  const deleted = getDeletedEventIds([
    { kind: 5, tags: [["e", ROOT]] },
    {
      kind: 9005,
      tags: [
        ["h", "chan"],
        ["e", MAIN],
      ],
    },
    { kind: 7, tags: [["e", OTHER]] },
  ]);
  assert.deepEqual([...deleted].sort(), [ROOT, MAIN].sort());
});

test("a deleted thread root takes its replies' rows with it", () => {
  const result = withoutDeletedFeedItems(
    feed(
      [
        item(REPLY, [
          ["e", ROOT, "", "root"],
          ["e", ROOT, "", "reply"],
        ]),
      ],
      [item(MAIN), item(OTHER)],
    ),
    new Set([ROOT, MAIN]),
  );
  assert.deepEqual(result.feed.mentions, []);
  assert.deepEqual(
    result.feed.activity.map((entry) => entry.id),
    [OTHER],
  );
});

test("no deletions keeps the same feed object", () => {
  const original = feed([item(MAIN)]);
  assert.equal(withoutDeletedFeedItems(original, new Set()), original);
});
