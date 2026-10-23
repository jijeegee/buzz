import assert from "node:assert/strict";
import test from "node:test";

import {
  compareGroupMatches,
  findThreadNameMatches,
  sortNewestFirst,
} from "./searchRanking.ts";

function threadRow(rootId, threadName, overrides = {}) {
  return {
    type: "thread",
    key: rootId,
    at: 100,
    channel_id: "kuzz-id",
    channel_name: "kuzz",
    root_id: rootId,
    thread_name: threadName,
    ...overrides,
  };
}

const anyChannel = () => true;

test("a Korean word in the middle of a thread name matches", () => {
  const matches = findThreadNameMatches(
    [threadRow("root-1", "채널 검색 및 초대")],
    "검색",
    anyChannel,
  );
  assert.equal(matches.length, 1);
  assert.deepEqual(matches[0].thread, {
    at: 100,
    channelId: "kuzz-id",
    channelName: "kuzz",
    closed: false,
    name: "채널 검색 및 초대",
    rootId: "root-1",
  });
});

test("relay search rows add closed threads the Chats list left out", () => {
  const chats = [threadRow("live", "Launch plan")];
  const searched = [
    threadRow("live", "Launch plan", { closed: true, match: 1 }),
    threadRow("done", "Old launch notes", { closed: true, match: 2 }),
  ];
  const matches = findThreadNameMatches(
    [...chats, ...searched],
    "launch",
    anyChannel,
  );
  assert.deepEqual(
    matches.map((m) => [m.thread.rootId, m.thread.closed]),
    [
      // The Chats row is listed first and wins.
      ["live", false],
      ["done", true],
    ],
  );
});

test("a substring inside a word matches, fuzzy-only hits do not", () => {
  const rows = [threadRow("a", "초대검색창"), threadRow("b", "release-notes")];
  assert.deepEqual(
    findThreadNameMatches(rows, "검색", anyChannel).map((m) => m.thread.rootId),
    ["a"],
  );
  // `reln` is only an in-order subsequence of `release-notes`.
  assert.deepEqual(findThreadNameMatches(rows, "reln", anyChannel), []);
});

test("rooms, unnamed threads, duplicates and hidden channels are skipped", () => {
  const rows = [
    threadRow("room", "검색", { type: "room" }),
    threadRow("unnamed", null),
    threadRow("blank", "   "),
    threadRow("dup", "검색 1"),
    threadRow("dup", "검색 2"),
    threadRow("hidden", "검색", { channel_id: "gone" }),
  ];
  const matches = findThreadNameMatches(
    rows,
    "검색",
    (channelId) => channelId !== "gone",
  );
  assert.deepEqual(
    matches.map((m) => [m.thread.rootId, m.thread.name]),
    [["dup", "검색 1"]],
  );
});

test("match quality ranks exact > prefix > word start > middle", () => {
  const rows = [
    threadRow("middle", "채널검색"),
    threadRow("word", "채널 검색"),
    threadRow("prefix", "검색 및 초대"),
    threadRow("exact", "검색"),
  ];
  const ranked = findThreadNameMatches(rows, "검색", anyChannel)
    .map(({ score, thread }) => ({
      activity: 0,
      joined: true,
      label: thread.name,
      rootId: thread.rootId,
      score,
    }))
    .sort(compareGroupMatches)
    .map((match) => match.rootId);
  assert.deepEqual(ranked, ["exact", "prefix", "word", "middle"]);
});

test("equal matches rank by recent activity; unjoined groups go last", () => {
  const matches = [
    { activity: 5, joined: false, label: "open exact", score: 0 },
    { activity: 1, joined: true, label: "old", score: 2 },
    { activity: 9, joined: true, label: "new", score: 2 },
    { activity: 0, joined: true, label: "prefix", score: 1 },
  ];
  assert.deepEqual(
    [...matches].sort(compareGroupMatches).map((match) => match.label),
    ["prefix", "new", "old", "open exact"],
  );
});

test("messages sort newest first and keep server order on ties", () => {
  const hits = [
    { eventId: "a", createdAt: 10 },
    { eventId: "b", createdAt: 30 },
    { eventId: "c", createdAt: 10 },
    { eventId: "d", createdAt: 20 },
  ];
  assert.deepEqual(
    sortNewestFirst(hits).map((hit) => hit.eventId),
    ["b", "d", "a", "c"],
  );
});
