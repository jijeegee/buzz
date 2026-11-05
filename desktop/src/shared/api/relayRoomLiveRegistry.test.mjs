import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  KIND_GOAL_OPS,
  RoomLiveRegistry,
  roomLiveRequestFilters,
} from "./relayRoomLiveRegistry.ts";

const fixture = JSON.parse(
  readFileSync(
    new URL(
      "../../../../test-fixtures/room-live-request.json",
      import.meta.url,
    ),
    "utf8",
  ),
);

const flush = async () => {
  for (let i = 0; i < 20; i++) await Promise.resolve();
};

/** A registry over a fake relay: every REQ opens at once unless held. */
function harness({ hold = false } = {}) {
  const opened = [];
  const registry = new RoomLiveRegistry((filters, onEvent, onGone) => {
    const req = { filters, onEvent, onGone, closed: false };
    opened.push(req);
    return new Promise((resolve, reject) => {
      req.open = () =>
        resolve(async () => {
          req.closed = true;
        });
      req.fail = (error) => reject(error);
      if (!hold) req.open();
    });
  });
  const open = () => opened.filter((req) => !req.closed);
  return { registry, opened, open };
}

const ROOM = fixture.room;
const event = (kind, id = `${kind}`) => ({
  id,
  kind,
  pubkey: "b".repeat(64),
  content: "",
  created_at: 1,
  tags: [["h", ROOM]],
  sig: "",
});

test("the REQ's filters match the shared fixture", () => {
  assert.deepEqual(roomLiveRequestFilters(ROOM), fixture.filters);
});

test("two listeners of one room share one REQ and get only their kind", async () => {
  const { registry, opened } = harness();
  const typing = [];
  const goals = [];
  registry.listen(ROOM, 20002, (e) => typing.push(e.id));
  registry.listen(ROOM, KIND_GOAL_OPS, (e) => goals.push(e.id));
  await flush();
  assert.equal(opened.length, 1);
  assert.deepEqual(opened[0].filters, fixture.filters);
  opened[0].onEvent(event(20002, "t"));
  opened[0].onEvent(event(KIND_GOAL_OPS, "g"));
  assert.deepEqual(typing, ["t"]);
  assert.deepEqual(goals, ["g"]);
});

test("a remount in the same task keeps the REQ: no CLOSE, no new REQ", async () => {
  const { registry, opened, open } = harness();
  const stop = registry.listen(ROOM, 20002, () => {});
  await flush();
  stop();
  registry.listen(ROOM, 20002, () => {});
  await flush();
  assert.equal(opened.length, 1);
  assert.equal(open().length, 1);
});

test("the last listener leaving closes the REQ one microtask later", async () => {
  const { registry, open } = harness();
  const stopA = registry.listen(ROOM, 20002, () => {});
  const stopB = registry.listen(ROOM, KIND_GOAL_OPS, () => {});
  await flush();
  stopA();
  await flush();
  assert.equal(open().length, 1, "one listener is left");
  stopB();
  stopB();
  await flush();
  assert.equal(open().length, 0);
  assert.deepEqual(registry.openRooms(), []);
});

test("stopping before the REQ is up closes it when it arrives", async () => {
  const { registry, opened } = harness({ hold: true });
  const stop = registry.listen(ROOM, 20002, () => {});
  stop();
  await flush();
  opened[0].open();
  await flush();
  assert.equal(opened[0].closed, true);
});

test("resync calls onResync once per listener", async () => {
  const { registry } = harness();
  let goals = 0;
  let other = 0;
  registry.listen(ROOM, KIND_GOAL_OPS, () => {}, {
    onResync: () => {
      goals += 1;
    },
  });
  registry.listen("other-room", KIND_GOAL_OPS, () => {}, {
    onResync: () => {
      other += 1;
    },
  });
  registry.listen(ROOM, 20002, () => {});
  await flush();
  registry.resync();
  assert.equal(goals, 1);
  assert.equal(other, 1);
});

test("a terminal CLOSED calls onGone and a later listener opens a fresh REQ", async () => {
  const { registry, opened } = harness();
  let gone = 0;
  registry.listen(ROOM, 20002, () => {}, {
    onGone: () => {
      gone += 1;
    },
  });
  await flush();
  opened[0].onGone();
  assert.equal(gone, 1);
  assert.deepEqual(registry.openRooms(), []);
  registry.listen(ROOM, 20002, () => {});
  await flush();
  assert.equal(opened.length, 2);
});

test("only the open-room kinds ride the REQ", () => {
  const { registry } = harness();
  assert.throws(() => registry.listen(ROOM, 9, () => {}));
});
