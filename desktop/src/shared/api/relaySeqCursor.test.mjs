import assert from "node:assert/strict";
import test from "node:test";

import {
  afterSeqBounds,
  frameChannelSequence,
  isAfterSeqBoundsEvent,
  orderBySeqWithinTime,
  SEQ_STABLE_AFTER_MS,
  SeqCursor,
  withKuzzSeqParam,
} from "./relaySeqCursor.ts";

const CH = "270f6caf-0feb-4055-93f3-cdbeb567ff28";

function event(id, createdAt, extra = {}) {
  return {
    id,
    pubkey: "p",
    created_at: createdAt,
    kind: 9,
    tags: [["h", CH]],
    content: "",
    sig: "s",
    ...extra,
  };
}

function bounds(content, d = `${CH}:after_seq:0`) {
  return event("bounds", 0, {
    kind: 39006,
    tags: [
      ["d", d],
      ["h", CH],
    ],
    content: JSON.stringify(content),
  });
}

test("a young cursor replays from just below the lowest number seen", () => {
  const cursor = new SeqCursor();
  assert.equal(cursor.replayPoint(0), undefined, "nothing seen: time path");
  cursor.observe(10, 0);
  cursor.observe(12, 1_000);
  assert.equal(cursor.replayPoint(2_000), 9);
});

test("a number that stayed newest for 30s becomes the replay point", () => {
  const cursor = new SeqCursor();
  cursor.observe(10, 0);
  cursor.observe(12, 1_000); // 10 was newest at 1s
  cursor.observe(15, 5_000);
  assert.equal(cursor.replayPoint(1_000 + SEQ_STABLE_AFTER_MS - 1), 9);
  assert.equal(cursor.replayPoint(1_000 + SEQ_STABLE_AFTER_MS), 10);
  // Later observations promote newer candidates once they age.
  cursor.observe(20, 1_000 + SEQ_STABLE_AFTER_MS); // candidate 15
  assert.equal(cursor.replayPoint(1_000 + 2 * SEQ_STABLE_AFTER_MS), 15);
});

test("a pin holds the replay point until cleared, lowest pin wins", () => {
  const cursor = new SeqCursor();
  cursor.observe(10, 0);
  cursor.observe(50, 1_000);
  const later = 1_000 + SEQ_STABLE_AFTER_MS;
  assert.equal(cursor.replayPoint(later), 10);
  cursor.pinAt(4);
  cursor.pinAt(7);
  assert.equal(cursor.replayPoint(later), 4);
  cursor.movePin(30);
  assert.equal(cursor.replayPoint(later), 10, "never later than the base");
  cursor.clearPin();
  assert.equal(cursor.replayPoint(later), 10);
});

test("frame metadata yields only non-negative integer numbers", () => {
  assert.equal(frameChannelSequence({ channel_sequence: 7 }), 7);
  assert.equal(frameChannelSequence({ channel_sequence: 0 }), 0);
  for (const meta of [
    undefined,
    null,
    "7",
    {},
    { channel_sequence: -1 },
    { channel_sequence: 1.5 },
    { channel_sequence: "7" },
  ]) {
    assert.equal(frameChannelSequence(meta), undefined);
  }
});

test("the seq query goes on the socket URL only", () => {
  assert.equal(
    withKuzzSeqParam("wss://gateway.kuzz.app"),
    "wss://gateway.kuzz.app?kuzz_seq=1",
  );
  assert.equal(
    withKuzzSeqParam("wss://h/path?x=1"),
    "wss://h/path?x=1&kuzz_seq=1",
  );
});

test("after_seq bounds parse; channel-window bounds do not", () => {
  const page = bounds({
    order: "seq",
    after_seq: 0,
    last_seq: 12,
    has_more: true,
    sequences: [
      ["a", 11],
      ["b", 12],
    ],
  });
  const parsed = afterSeqBounds(page);
  assert.equal(parsed?.lastSeq, 12);
  assert.equal(parsed?.hasMore, true);
  assert.deepEqual([...(parsed?.sequences ?? [])], [
    ["a", 11],
    ["b", 12],
  ]);
  assert.equal(isAfterSeqBoundsEvent(page), true);

  const window = bounds({ has_more: true }, `${CH}:window`);
  assert.equal(afterSeqBounds(window), undefined);
  assert.equal(isAfterSeqBoundsEvent(window), false);
  assert.equal(isAfterSeqBoundsEvent(event("m", 1)), false);
});

test("display order uses numbers among numbered events, time otherwise", () => {
  const old = event("old", 100);
  const a = event("a", 105, { seq: 3 }); // sender clock ahead
  const page = event("page", 101);
  const b = event("b", 103, { seq: 2 });
  const byTime = [old, page, b, a];
  assert.deepEqual(
    orderBySeqWithinTime(byTime).map((e) => e.id),
    ["old", "page", "b", "a"],
    "already in number order: unchanged",
  );
  const c = event("c", 102, { seq: 5 });
  const ordered = orderBySeqWithinTime([old, page, c, b, a]);
  assert.deepEqual(
    ordered.map((e) => e.id),
    ["old", "page", "b", "a", "c"],
    "numbered events keep their slots and sort by number",
  );
  const noSeq = [old, page];
  assert.equal(orderBySeqWithinTime(noSeq), noSeq);
});
