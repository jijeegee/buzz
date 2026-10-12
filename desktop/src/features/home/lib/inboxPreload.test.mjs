import assert from "node:assert/strict";
import test from "node:test";

import { inboxPreloadTargetKey, inboxPreloadTargets } from "./inboxPreload.ts";

const ROOM = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
const DM = "8ad375a7-6990-4b22-985f-e3fd34f634d7";
const ROOT = "c".repeat(64);

function row(conversationId, channelId, channelType, tags = []) {
  const item = {
    category: "activity",
    channelId,
    channelName: "x",
    channelType,
    content: "hi",
    createdAt: 100,
    id: `${conversationId}-latest`,
    kind: 9,
    pubkey: "a".repeat(64),
    tags,
  };
  return {
    categories: ["activity"],
    conversationId,
    groupItems: [item],
    id: item.id,
    isActionRequired: false,
    item,
    unreadCount: 0,
  };
}

test("targets follow row order: rooms, DMs and threads", () => {
  const targets = inboxPreloadTargets([
    row(ROOT, ROOM, "stream", [["e", ROOT, "", "reply"]]),
    row(`channel:${ROOM}`, ROOM, "stream"),
    row(`dm:${DM}`, DM, "dm"),
  ]);
  assert.deepEqual(targets, [
    { channelId: ROOM, thread: { replyId: `${ROOT}-latest`, rootId: ROOT } },
    { channelId: ROOM, thread: null },
    { channelId: DM, thread: null },
  ]);
  assert.deepEqual(targets.map(inboxPreloadTargetKey), [
    `${ROOM}:${ROOT}`,
    ROOM,
    DM,
  ]);
});

test("only the first rows are preloaded and non-chat rows are skipped", () => {
  const rows = Array.from({ length: 20 }, (_, index) =>
    row(`channel:${index}`, `room-${index}`, "stream"),
  );
  const approval = { ...row("x", ROOM, "stream"), isActionRequired: true };
  assert.equal(inboxPreloadTargets(rows).length, 12);
  assert.deepEqual(inboxPreloadTargets([approval]), []);
});

test("a single top-level message row is not treated as a thread", () => {
  const single = row("e".repeat(64), ROOM, "stream");
  assert.deepEqual(inboxPreloadTargets([single]), [
    { channelId: ROOM, thread: null },
  ]);
});

test("a DM thread row preloads the reply's real thread root", () => {
  const reply = row(`dm:${DM}`, DM, "dm", [["e", ROOT, "", "reply"]]);
  assert.deepEqual(inboxPreloadTargets([reply]), [
    { channelId: DM, thread: { replyId: `dm:${DM}-latest`, rootId: ROOT } },
  ]);
});

test("the second wave takes the rows below the first", () => {
  const rows = Array.from({ length: 60 }, (_, index) =>
    row(`channel:${index}`, `room-${index}`, "stream"),
  );
  const more = inboxPreloadTargets(rows, 36, 12);
  assert.equal(more.length, 36);
  assert.equal(more[0].channelId, "room-12");
  assert.equal(more.at(-1).channelId, "room-47");
});
