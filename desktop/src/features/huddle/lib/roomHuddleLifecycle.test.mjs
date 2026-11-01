import assert from "node:assert/strict";
import test from "node:test";

import { RoomHuddleLifecycle } from "./roomHuddleLifecycle.ts";

const flush = async () => {
  for (let i = 0; i < 20; i++) await Promise.resolve();
};

const ROOM = "room-a";
const ev = (id, kind = 48101, created_at = 10) => ({
  id,
  kind,
  pubkey: "b".repeat(64),
  content: "",
  created_at,
  tags: [["h", ROOM]],
  sig: "",
});

function harness({ history = [] } = {}) {
  const listens = [];
  const fetches = [];
  const lifecycle = new RoomHuddleLifecycle({
    listenRoom: async (roomId, onEvent) => {
      const listen = { roomId, onEvent, stopped: false };
      listens.push(listen);
      return async () => {
        listen.stopped = true;
      };
    },
    fetchEvents: (filter) => {
      fetches.push(filter);
      return Promise.resolve(history);
    },
  });
  return { lifecycle, listens, fetches };
}

test("an indicator and three cards share one room listener and one history read", async () => {
  const { lifecycle, listens, fetches } = harness({
    history: [ev("h1", 48100)],
  });
  const seen = [[], [], [], []];
  for (const ids of seen) lifecycle.listen(ROOM, (e) => ids.push(e.id));
  await flush();
  assert.equal(listens.length, 1);
  assert.equal(fetches.length, 1);
  assert.deepEqual(fetches[0], {
    kinds: [48100, 48101, 48102, 48103],
    "#h": [ROOM],
    limit: 100,
  });
  for (const ids of seen) assert.deepEqual(ids, ["h1"]);
});

test("only lifecycle kinds pass, and an event from both live and history is delivered once", async () => {
  const { lifecycle, listens } = harness({ history: [ev("gap")] });
  const ids = [];
  lifecycle.listen(ROOM, (e) => ids.push(e.id));
  await flush();
  listens[0].onEvent(ev("gap"));
  listens[0].onEvent(ev("chat", 9));
  listens[0].onEvent(ev("new", 48102, 20));
  assert.deepEqual(ids, ["gap", "new"]);
});

test("a later listener gets every known event at once", async () => {
  const { lifecycle, listens } = harness({ history: [ev("a", 48100)] });
  lifecycle.listen(ROOM, () => {});
  await flush();
  listens[0].onEvent(ev("b"));
  const late = [];
  lifecycle.listen(ROOM, (e) => late.push(e.id));
  assert.deepEqual(late.sort(), ["a", "b"]);
});

test("the room stops listening after its last listener, not on a same-task remount", async () => {
  const { lifecycle, listens } = harness();
  const stop = lifecycle.listen(ROOM, () => {});
  await flush();
  stop();
  const stopAgain = lifecycle.listen(ROOM, () => {});
  await flush();
  assert.equal(listens.length, 1);
  assert.equal(listens[0].stopped, false);
  stopAgain();
  await flush();
  assert.equal(listens[0].stopped, true);
  lifecycle.listen(ROOM, () => {});
  await flush();
  assert.equal(listens.length, 2, "a new listener starts the room again");
});
