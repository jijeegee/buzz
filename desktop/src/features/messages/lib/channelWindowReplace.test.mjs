import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  appendOlderChannelWindow,
  emptyChannelWindowStore,
  mergeLiveThreadSummary,
  replaceNewestChannelWindow,
} from "./channelWindowStore.ts";

// Shared rules with mobile (test-fixtures/channel-window-replace.json).
const fixture = JSON.parse(
  readFileSync(
    new URL(
      "../../../../../test-fixtures/channel-window-replace.json",
      import.meta.url,
    ),
    "utf8",
  ),
);

const message = ([id, createdAt]) => ({
  id,
  pubkey: "alice",
  created_at: createdAt,
  kind: 9,
  tags: [["h", "room"]],
  content: "",
  sig: "",
});
const reaction = ([id, target]) => ({
  id,
  pubkey: "alice",
  created_at: 1,
  kind: 7,
  tags: [
    ["h", "room"],
    ["e", target],
  ],
  content: "+",
  sig: "",
});
const summary = {
  replyCount: 1,
  descendantCount: 1,
  lastReplyAt: null,
  participantPubkeys: [],
};
const cursorOf = (row) => (row ? { createdAt: row[1], eventId: row[0] } : null);

function page(spec, startCursor) {
  return {
    startCursor,
    rows: spec.rows.map((row) => ({
      event: message(row),
      thread: spec.threads?.[row[0]] === undefined ? null : summary,
    })),
    aux: (spec.aux ?? []).map(reaction),
    nextCursor: spec.more ? cursorOf(spec.rows.at(-1)) : null,
    hasMore: spec.more,
  };
}

function storeOf(testCase) {
  let store = emptyChannelWindowStore();
  testCase.current.forEach((spec, index) => {
    const startCursor =
      index === 0 ? null : store.pages[store.pages.length - 1].nextCursor;
    const next = page(spec, startCursor);
    store =
      index === 0
        ? replaceNewestChannelWindow(store, next)
        : appendOlderChannelWindow(store, next);
  });
  for (const rootId of testCase.live_summaries ?? []) {
    store = mergeLiveThreadSummary(store, rootId, { summary, createdAt: 1 });
  }
  return store;
}

const cursorJson = (cursor) =>
  cursor ? [cursor.createdAt, cursor.eventId] : null;

for (const testCase of fixture.cases) {
  test(`shared fixture: ${testCase.name}`, () => {
    let store = replaceNewestChannelWindow(
      storeOf(testCase),
      page(testCase.page, null),
    );
    if (testCase.then_older) {
      store = appendOlderChannelWindow(
        store,
        page(
          testCase.then_older,
          store.pages[store.pages.length - 1].nextCursor,
        ),
      );
    }
    assert.deepEqual(
      store.pages.map((retained) => ({
        start: cursorJson(retained.startCursor),
        rows: retained.rows.map((row) => row.event.id),
        aux: retained.aux.map((event) => event.id),
        next: cursorJson(retained.nextCursor),
      })),
      testCase.expect.pages,
    );
    if (testCase.expect.live_summaries) {
      assert.deepEqual(
        Object.keys(store.liveSummaries).sort(),
        testCase.expect.live_summaries,
      );
    }
  });
}
