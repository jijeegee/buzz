import assert from "node:assert/strict";
import test from "node:test";

import {
  buildQuoteTags,
  canQuoteMessage,
  getQuoteReference,
  QUOTE_TAG,
  quoteTargetFromMessage,
  quoteTargetTags,
  withQuoteTags,
} from "./messageQuote.ts";
import { getThreadReference } from "./threading.ts";

const EVENT_ID = "a".repeat(64);
const AUTHOR = "b".repeat(64);

function message(overrides = {}) {
  return {
    author: "Alice",
    body: "**Ship** the [launch plan](https://example.com) today",
    createdAt: 1,
    depth: 0,
    id: EVENT_ID,
    pubkey: AUTHOR,
    tags: [],
    time: "9:00",
    ...overrides,
  };
}

test("buildQuoteTags emits the NIP-18 q tag with an empty relay hint", () => {
  assert.deepEqual(buildQuoteTags(EVENT_ID.toUpperCase(), ` ${AUTHOR} `), [
    [QUOTE_TAG, EVENT_ID, "", AUTHOR],
  ]);
  assert.deepEqual(buildQuoteTags(EVENT_ID, AUTHOR, "wss://relay.example"), [
    ["q", EVENT_ID, "wss://relay.example", AUTHOR],
  ]);
});

test("buildQuoteTags rejects malformed ids and pubkeys", () => {
  assert.throws(() => buildQuoteTags("not-hex", AUTHOR), /valid event ID/);
  assert.throws(() => buildQuoteTags(EVENT_ID, "short"), /valid author/);
});

test("a quote tag never contributes thread placement", () => {
  const topLevel = [["h", "channel"], ...buildQuoteTags(EVENT_ID, AUTHOR)];
  assert.deepEqual(getThreadReference(topLevel), {
    parentId: null,
    rootId: null,
  });
  const reply = [
    ["h", "channel"],
    ["e", "c".repeat(64), "", "reply"],
    ...buildQuoteTags(EVENT_ID, AUTHOR),
  ];
  assert.deepEqual(getThreadReference(reply), {
    parentId: "c".repeat(64),
    rootId: "c".repeat(64),
  });
});

test("getQuoteReference reads the first well-formed q tag", () => {
  assert.equal(getQuoteReference(undefined), null);
  assert.equal(getQuoteReference([["q", "bogus"]]), null);
  assert.deepEqual(
    getQuoteReference([
      ["q", "bogus"],
      ["q", EVENT_ID.toUpperCase(), "", AUTHOR],
    ]),
    { authorPubkey: AUTHOR, eventId: EVENT_ID, relayUrl: null },
  );
  assert.deepEqual(getQuoteReference([["q", EVENT_ID, "wss://r", "x"]]), {
    authorPubkey: null,
    eventId: EVENT_ID,
    relayUrl: "wss://r",
  });
});

test("quoteTargetFromMessage captures author and a one-line excerpt", () => {
  assert.deepEqual(quoteTargetFromMessage(message()), {
    author: "Alice",
    authorPubkey: AUTHOR,
    eventId: EVENT_ID,
    excerpt: "Ship the launch plan today",
  });
});

test("pending, system, and malformed messages cannot be quoted", () => {
  assert.equal(canQuoteMessage(message()), true);
  assert.equal(canQuoteMessage(message({ pending: true })), false);
  assert.equal(canQuoteMessage(message({ id: "optimistic-1" })), false);
  assert.equal(canQuoteMessage(message({ pubkey: undefined })), false);
  assert.equal(quoteTargetFromMessage(message({ pending: true })), null);
});

test("quoteTargetTags is empty without a target", () => {
  assert.deepEqual(quoteTargetTags(null), []);
  const target = quoteTargetFromMessage(message());
  assert.deepEqual(quoteTargetTags(target), [["q", EVENT_ID, "", AUTHOR]]);
});

test("withQuoteTags appends exactly one quote to the outgoing tags", () => {
  const imeta = ["imeta", "url https://blossom/a.png"];
  assert.equal(withQuoteTags(undefined, null), undefined);
  assert.deepEqual(withQuoteTags([imeta], null), [imeta]);
  const target = quoteTargetFromMessage(message());
  assert.deepEqual(withQuoteTags(undefined, target), [
    ["q", EVENT_ID, "", AUTHOR],
  ]);
  assert.deepEqual(
    withQuoteTags([imeta, ["q", "c".repeat(64), "", AUTHOR]], target),
    [imeta, ["q", EVENT_ID, "", AUTHOR]],
  );
});
