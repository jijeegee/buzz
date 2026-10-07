import assert from "node:assert/strict";
import test from "node:test";

import { createOptimisticMessage } from "../hooks.ts";
import { buildQuoteTags } from "./messageQuote.ts";
import { getThreadReference } from "./threading.ts";

// A quote rides the outgoing tag set; e-tags alone decide placement.
const IDENTITY = { pubkey: "1".repeat(64) };
const QUOTED = "a".repeat(64);
const AUTHOR = "b".repeat(64);
const ROOT = "c".repeat(64);

test("a quoted main-timeline message stays top-level", () => {
  const msg = createOptimisticMessage(
    "chan",
    "see above",
    IDENTITY,
    [],
    [],
    null,
    buildQuoteTags(QUOTED, AUTHOR),
  );
  assert.equal(
    msg.tags.some(([name]) => name === "e"),
    false,
  );
  assert.deepEqual(getThreadReference(msg.tags), {
    parentId: null,
    rootId: null,
  });
  assert.deepEqual(
    msg.tags.find(([name]) => name === "q"),
    ["q", QUOTED, "", AUTHOR],
  );
});

test("a quoted thread reply keeps only its thread e-tags", () => {
  const msg = createOptimisticMessage(
    "chan",
    "in thread",
    IDENTITY,
    [],
    [],
    ROOT,
    buildQuoteTags(QUOTED, AUTHOR),
  );
  assert.deepEqual(
    msg.tags.filter(([name]) => name === "e"),
    [["e", ROOT, "", "reply"]],
  );
  assert.deepEqual(getThreadReference(msg.tags), {
    parentId: ROOT,
    rootId: ROOT,
  });
  assert.ok(msg.tags.some(([name, id]) => name === "q" && id === QUOTED));
});
