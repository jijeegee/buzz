import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  decideOpenAnchor,
  reanchorTarget,
  roomPaintDecides,
  threadPaintDecides,
} from "./openAnchorDecision.ts";

// Shared rules with mobile (test-fixtures/message-store-paint-first.json).
const fixture = JSON.parse(
  readFileSync(
    new URL(
      "../../../../../test-fixtures/message-store-paint-first.json",
      import.meta.url,
    ),
    "utf8",
  ),
);

function decide(testCase) {
  const ids = new Set(testCase.rows.map(([id]) => id));
  return decideOpenAnchor({
    opensElsewhere: false,
    frontierSeconds: testCase.frontier,
    frontierUnknown: false,
    firstUnreadMessageId: testCase.first_unread,
    remembered: testCase.remembered
      ? { messageId: testCase.remembered, topPx: 0 }
      : null,
    hasRow: (id) => ids.has(id),
    oldestTopLevelAt: testCase.rows[0]?.[1] ?? null,
    hasOlderMessages: testCase.has_older,
    canSeekOlder: testCase.has_older,
    seekExpired: false,
  });
}

for (const testCase of fixture.anchor) {
  test(`anchor: ${testCase.name}`, () => {
    const decision = decide(testCase);
    if (testCase.expect.seek) {
      assert.equal(decision.kind, "seek");
      return;
    }
    assert.equal(decision.kind, "open");
    assert.equal(decision.anchorMessageId, testCase.expect.open);
    // Rows that are not current decide provisionally (useChannelOpenAnchor).
    assert.equal(!testCase.current, testCase.expect.provisional);
  });
}

for (const testCase of fixture.reanchor) {
  test(`reanchor: ${testCase.name}`, () => {
    const onCurrent = testCase.on_current.seek
      ? { kind: "seek" }
      : {
          kind: "open",
          anchorMessageId: testCase.on_current.open,
          anchorTopPx: null,
          keepsRemembered: false,
        };
    const next = reanchorTarget(
      testCase.provisional_open,
      onCurrent,
      testCase.reader_moved,
    );
    if (testCase.expect.seek) {
      assert.equal(next?.kind, "seek");
      return;
    }
    assert.equal(
      next === undefined ? testCase.provisional_open : next.anchorMessageId,
      testCase.expect.open,
    );
  });
}

for (const testCase of fixture.paint_gap) {
  test(`paint gap: ${testCase.name}`, () => {
    const decides = testCase.thread
      ? threadPaintDecides(
          testCase.complete_from,
          testCase.first_unread,
          testCase.read_before,
          testCase.has_older,
        )
      : roomPaintDecides(
          testCase.complete_from,
          testCase.first_unread === null ? null : "u1",
          testCase.frontier,
        );
    assert.equal(decides, testCase.expect.decides);
  });
}

test("rows that are not a store paint always decide", () => {
  assert.equal(roomPaintDecides(undefined, "u1", 150), true);
  assert.equal(threadPaintDecides(undefined, 200, null, false), true);
});

test("a link opens elsewhere and a frontier without the read state trusts no row", () => {
  const base = {
    opensElsewhere: false,
    frontierSeconds: 150,
    frontierUnknown: false,
    firstUnreadMessageId: null,
    remembered: { messageId: "r1", topPx: 30 },
    hasRow: () => true,
    oldestTopLevelAt: 100,
    hasOlderMessages: true,
    canSeekOlder: true,
    seekExpired: false,
  };
  assert.deepEqual(decideOpenAnchor({ ...base, opensElsewhere: true }), {
    kind: "open",
    anchorMessageId: null,
    anchorTopPx: null,
    keepsRemembered: false,
  });
  assert.deepEqual(decideOpenAnchor({ ...base, frontierUnknown: true }), {
    kind: "open",
    anchorMessageId: null,
    anchorTopPx: null,
    keepsRemembered: true,
  });
  assert.equal(decideOpenAnchor(base).anchorTopPx, 30);
});
