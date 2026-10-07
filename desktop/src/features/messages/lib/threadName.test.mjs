import assert from "node:assert/strict";
import test from "node:test";
import {
  isValidThreadName,
  selectThreadName,
  threadNameWeight,
} from "./threadName.ts";

test("thread names enforce English, Korean and mixed boundaries", () => {
  for (const name of [
    "",
    "a".repeat(40),
    "가".repeat(20),
    "가".repeat(10) + "a".repeat(20),
  ]) {
    assert.equal(isValidThreadName(name), true);
  }
  for (const name of [
    "a".repeat(41),
    "가".repeat(21),
    "가".repeat(20) + "a",
    " leading",
    "trailing ",
    "a\nb",
    "a\u2028b",
  ]) {
    assert.equal(isValidThreadName(name), false);
  }
  assert.equal(threadNameWeight("한글 test"), 9);
});

test("live and history names converge without stale overwrites or channel leakage", () => {
  const event = {
    id: "b",
    kind: 40009,
    created_at: 5,
    content: "작업",
    tags: [
      ["h", "channel"],
      ["e", "thread"],
    ],
  };
  const select = (head, next) =>
    selectThreadName(head, next, "channel", "thread");
  assert.equal(select(null, event), event);
  assert.equal(
    select(event, { ...event, created_at: 4, content: "old" }),
    event,
  );
  assert.equal(
    select(event, {
      ...event,
      created_at: 6,
      tags: [
        ["h", "other"],
        ["e", "thread"],
      ],
    }),
    event,
  );
  assert.equal(
    select(event, { ...event, created_at: 6, content: "x".repeat(41) }),
    event,
  );
  assert.equal(select(event, { ...event, created_at: 6, kind: 9 }), event);
  assert.equal(select(event, { ...event, id: "a" }).id, "a");
  assert.equal(
    select(event, { ...event, created_at: 6, content: "" }).content,
    "",
  );
});
