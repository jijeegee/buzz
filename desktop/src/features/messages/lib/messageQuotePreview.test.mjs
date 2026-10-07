import assert from "node:assert/strict";
import test from "node:test";

import {
  findLoadedQuotedEvent,
  quotedPreviewFromEvent,
} from "./messageQuotePreview.ts";

const QUOTED_ID = "a".repeat(64);
const AUTHOR = "b".repeat(64);
const SIGNER = "c".repeat(64);
const ROOT = "d".repeat(64);

function event(overrides = {}) {
  return {
    content: "**Launch** plan for Friday",
    created_at: 1,
    id: QUOTED_ID,
    kind: 9,
    pubkey: SIGNER,
    sig: "",
    tags: [["h", "chan"]],
    ...overrides,
  };
}

test("findLoadedQuotedEvent searches every loaded cache", () => {
  const target = event();
  assert.equal(findLoadedQuotedEvent(QUOTED_ID, [undefined, []]), null);
  assert.equal(
    findLoadedQuotedEvent(QUOTED_ID, [[event({ id: ROOT })], [target]]),
    target,
  );
});

test("quotedPreviewFromEvent names the quoted author from the q tag", () => {
  const preview = quotedPreviewFromEvent(
    { authorPubkey: AUTHOR, eventId: QUOTED_ID, relayUrl: null },
    event(),
    { [AUTHOR]: { displayName: "Alice" } },
  );
  assert.deepEqual(preview, {
    kind: "ready",
    author: "Alice",
    excerpt: "Launch plan for Friday",
    threadRootId: null,
  });
});

test("quotedPreviewFromEvent records the thread a quoted reply lives in", () => {
  const preview = quotedPreviewFromEvent(
    { authorPubkey: null, eventId: QUOTED_ID, relayUrl: null },
    event({
      tags: [
        ["h", "chan"],
        ["e", ROOT, "", "reply"],
      ],
    }),
    { [SIGNER]: { displayName: "Bot" } },
  );
  assert.equal(preview.kind, "ready");
  assert.equal(preview.author, "Bot");
  assert.equal(preview.threadRootId, ROOT);
});
