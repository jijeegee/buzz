import assert from "node:assert/strict";
import test from "node:test";

import { splitChatWorking } from "./chatWorking.ts";

const thread = (channel_id, root_id) => ({
  type: "thread",
  channel_id,
  root_id,
  key: root_id,
});

test("work in a listed thread shows on the thread row only", () => {
  const byChannel = new Map([
    [
      "c1",
      {
        channelId: "c1",
        anchorAt: 100,
        agentCount: 2,
        agentPubkeys: ["a", "b"],
        agentNames: ["Fizz", "Honey"],
        turns: [
          { agentPubkey: "a", threadRootEventId: "t1", anchorAt: 100 },
          { agentPubkey: "b", threadRootEventId: null, anchorAt: 200 },
        ],
      },
    ],
  ]);
  const { rooms, threads } = splitChatWorking(byChannel, [thread("c1", "t1")]);
  assert.deepEqual(threads.get("t1")?.agentNames, ["Fizz"]);
  assert.equal(threads.get("t1")?.anchorAt, 100);
  assert.deepEqual(rooms.get("c1")?.agentNames, ["Honey"]);
  assert.equal(rooms.get("c1")?.anchorAt, 200);
});

test("a thread not in the list and typing-only work stay on the room", () => {
  const byChannel = new Map([
    [
      "c1",
      {
        channelId: "c1",
        anchorAt: 50,
        agentCount: 2,
        agentPubkeys: ["a", "b"],
        agentNames: ["Fizz", "Honey"],
        turns: [{ agentPubkey: "a", threadRootEventId: "t9", anchorAt: 80 }],
      },
    ],
    [
      "dm",
      { channelId: "dm", anchorAt: 10, agentCount: 1, agentPubkeys: ["c"] },
    ],
  ]);
  const { rooms, threads } = splitChatWorking(byChannel, [thread("c1", "t1")]);
  assert.equal(threads.size, 0);
  assert.deepEqual(rooms.get("c1")?.agentPubkeys, ["a", "b"]);
  assert.equal(rooms.get("dm")?.agentCount, 1);
});
