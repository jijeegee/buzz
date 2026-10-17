import assert from "node:assert/strict";
import test from "node:test";
import {
  hasThreadClosedTag,
  isTaskThreadClosed,
  threadCloseState,
} from "./taskThread.ts";

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

test("the close toggle names the closer while closed", () => {
  const mine = { ...closed(2), pubkey: "mina" };
  assert.deepEqual(threadCloseState([reply(1), mine]), {
    closed: true,
    closedBy: "mina",
    marker: mine,
  });
});

test("a reopen marker or any later reply turns the toggle back off", () => {
  const reopen = {
    createdAt: 3,
    pubkey: "joe",
    tags: [["buzz:thread-reopened", "a".repeat(64)]],
  };
  assert.deepEqual(threadCloseState([closed(2), reopen]), {
    closed: false,
    closedBy: null,
    marker: null,
  });
  assert.equal(threadCloseState([closed(2), reply(3)]).closed, false);
  assert.equal(threadCloseState([closed(2)]).closedBy, null);
});

test("an Undo in the same second as Close reads as open in any order", () => {
  const reopen = {
    createdAt: 2,
    tags: [["buzz:thread-reopened", "a".repeat(64)]],
  };
  assert.equal(threadCloseState([closed(2), reopen]).closed, false);
  assert.equal(threadCloseState([reopen, closed(2)]).closed, false);
});
