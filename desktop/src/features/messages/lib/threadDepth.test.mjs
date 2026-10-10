import assert from "node:assert/strict";
import test from "node:test";

import { canReplyInThread, MAX_THREAD_DEPTH } from "./threadDepth.ts";

test("threads are one level deep", () => {
  assert.equal(MAX_THREAD_DEPTH, 1);
});

test("only messages shallower than the limit can be replied to in thread", () => {
  assert.equal(canReplyInThread(0), true);
  assert.equal(canReplyInThread(MAX_THREAD_DEPTH - 1), true);
  assert.equal(canReplyInThread(MAX_THREAD_DEPTH), false);
  assert.equal(canReplyInThread(MAX_THREAD_DEPTH + 1), false);
});
