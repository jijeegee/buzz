import assert from "node:assert/strict";
import test from "node:test";

import { withoutBlockedFeedItems } from "./blockedFeed.ts";

const item = (id, pubkey) => ({ id, pubkey });
const response = (feed) => ({
  feed: {
    mentions: [],
    needsAction: [],
    activity: [],
    agentActivity: [],
    ...feed,
  },
  meta: { since: 0 },
});

test("a blocked person's mentions and activity leave the feed", () => {
  const feed = response({
    mentions: [item("m1", "bad"), item("m2", "ok")],
    activity: [item("a1", "bad")],
    needsAction: [item("n1", "ok")],
  });
  const result = withoutBlockedFeedItems(feed, (pk) => pk === "bad");
  assert.deepEqual(
    result.feed.mentions.map((i) => i.id),
    ["m2"],
  );
  assert.deepEqual(result.feed.activity, []);
  assert.equal(result.feed.needsAction, feed.feed.needsAction);
  assert.equal(result.meta, feed.meta);
});

test("with nobody blocked the same response comes back", () => {
  const feed = response({ mentions: [item("m1", "ok")] });
  assert.equal(
    withoutBlockedFeedItems(feed, () => false),
    feed,
  );
});
