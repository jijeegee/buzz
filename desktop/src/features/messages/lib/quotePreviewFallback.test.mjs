import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";

import {
  fetchQuotePreviewFallback,
  parseQuotePreviewEvent,
  resetQuotePreviewCache,
} from "./quotePreviewFallback.ts";

const CHANNEL = "chan-1";
const QUOTED = "a".repeat(64);
const QUOTING = "b".repeat(64);
const AUTHOR = "c".repeat(64);
const REQUEST = { channelId: CHANNEL, quotedId: QUOTED, quotingId: QUOTING };

function previewEvent(overrides = {}) {
  return {
    content: JSON.stringify({ excerpt: "Plan for Friday…", kind: 9 }),
    created_at: 1,
    id: "f".repeat(64),
    kind: 39008,
    pubkey: "e".repeat(64),
    sig: "",
    tags: [
      ["e", QUOTED],
      ["d", QUOTED],
      ["h", CHANNEL],
      ["p", AUTHOR],
    ],
    ...overrides,
  };
}

beforeEach(() => resetQuotePreviewCache());

test("a matching preview yields the author and excerpt", () => {
  assert.deepEqual(parseQuotePreviewEvent(previewEvent(), CHANNEL, QUOTED), {
    authorPubkey: AUTHOR,
    excerpt: "Plan for Friday…",
  });
});

test("a preview for another message, room, kind or body is ignored", () => {
  assert.equal(parseQuotePreviewEvent(null, CHANNEL, QUOTED), null);
  assert.equal(
    parseQuotePreviewEvent(previewEvent({ kind: 9 }), CHANNEL, QUOTED),
    null,
  );
  assert.equal(
    parseQuotePreviewEvent(previewEvent(), "other-room", QUOTED),
    null,
  );
  assert.equal(
    parseQuotePreviewEvent(previewEvent(), CHANNEL, "d".repeat(64)),
    null,
  );
  assert.equal(
    parseQuotePreviewEvent(
      previewEvent({ content: "not json" }),
      CHANNEL,
      QUOTED,
    ),
    null,
  );
  assert.equal(
    parseQuotePreviewEvent(
      previewEvent({ content: JSON.stringify({ excerpt: "  " }) }),
      CHANNEL,
      QUOTED,
    ),
    null,
  );
});

test("the fallback asks once per channel, quoted and quoting message", async () => {
  const requests = [];
  const deps = {
    fetchPreview: async (request) => {
      requests.push(request);
      return previewEvent();
    },
    fetchAuthorName: async () => "Alice",
  };
  const first = await fetchQuotePreviewFallback(REQUEST, deps);
  const again = await fetchQuotePreviewFallback(REQUEST, deps);
  await fetchQuotePreviewFallback(
    { ...REQUEST, quotingId: "9".repeat(64) },
    deps,
  );

  assert.deepEqual(first, {
    kind: "ready",
    authorName: "Alice",
    authorPubkey: AUTHOR,
    excerpt: "Plan for Friday…",
  });
  assert.equal(again, first);
  assert.equal(requests.length, 2);
  assert.deepEqual(requests[0], REQUEST);
});

test("no preview from the relay means the original stays unavailable", async () => {
  let calls = 0;
  const deps = {
    fetchPreview: async () => {
      calls += 1;
      return null;
    },
    fetchAuthorName: async () => "Alice",
  };
  assert.deepEqual(await fetchQuotePreviewFallback(REQUEST, deps), {
    kind: "none",
  });
  await fetchQuotePreviewFallback(REQUEST, deps);
  assert.equal(calls, 1, "a definitive empty answer is cached");
});

test("a failed request is retried on the next render", async () => {
  let calls = 0;
  const deps = {
    fetchPreview: async () => {
      calls += 1;
      if (calls === 1) throw new Error("offline");
      return previewEvent();
    },
    fetchAuthorName: async () => {
      throw new Error("no profile");
    },
  };
  assert.deepEqual(await fetchQuotePreviewFallback(REQUEST, deps), {
    kind: "none",
  });
  await new Promise((resolve) => setTimeout(resolve, 0));
  const retried = await fetchQuotePreviewFallback(REQUEST, deps);
  assert.equal(retried.kind, "ready");
  assert.equal(retried.authorName, null);
  assert.equal(calls, 2);
});
