import assert from "node:assert/strict";
import test from "node:test";
import {
  hasThreadClosedTag,
  isTaskThreadClosed,
  isThreadMarkerReply,
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

test("a plain reply and a close marker in one second: the later in order wins", () => {
  const reply = { createdAt: 2, tags: [] };
  // Agent's final reply, then `kuzz threads close` in the same second.
  assert.equal(threadCloseState([reply, closed(2)]).closed, true);
  assert.equal(threadCloseState([closed(2), reply]).closed, false);
});

test("only marker replies are thread markers, not a top-level close result", () => {
  const closed = [["buzz:thread-closed", "root"]];
  assert.equal(isThreadMarkerReply({ parentId: "root", tags: closed }), true);
  assert.equal(
    isThreadMarkerReply({
      parentId: "root",
      tags: [["buzz:thread-reopened", "root"]],
    }),
    true,
  );
  // `kuzz threads close` also tags its result on the main timeline.
  assert.equal(isThreadMarkerReply({ parentId: null, tags: closed }), false);
  assert.equal(isThreadMarkerReply({ parentId: "root", tags: [] }), false);
});
