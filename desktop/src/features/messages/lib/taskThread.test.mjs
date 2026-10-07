import assert from "node:assert/strict";
import test from "node:test";
import { hasThreadClosedTag, isTaskThreadClosed } from "./taskThread.ts";

const closed = (createdAt) => ({
  createdAt,
  tags: [["buzz:thread-closed", "a".repeat(64)]],
});
const reply = (createdAt) => ({ createdAt, tags: [["e", "a".repeat(64)]] });

test("a thread is closed while its latest reply is the close marker", () => {
  assert.equal(isTaskThreadClosed([reply(1), closed(2)]), true);
  assert.equal(isTaskThreadClosed([closed(2), reply(1)]), true);
});

test("a later message reopens a closed thread", () => {
  assert.equal(isTaskThreadClosed([reply(1), closed(2), reply(3)]), false);
  assert.equal(isTaskThreadClosed([closed(2), reply(2)]), false);
});

test("threads without replies or markers are open", () => {
  assert.equal(isTaskThreadClosed([]), false);
  assert.equal(isTaskThreadClosed([reply(1)]), false);
  assert.equal(hasThreadClosedTag([["buzz:thread-closed"]]), false);
  assert.equal(hasThreadClosedTag(undefined), false);
});
