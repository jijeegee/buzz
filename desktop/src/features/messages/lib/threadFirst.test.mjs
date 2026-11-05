import assert from "node:assert/strict";
import { test } from "node:test";

import { afterThreadRequest, noteThreadRequest } from "./threadFirst.ts";

test("a room with no thread opening goes at once", async () => {
  await afterThreadRequest("no-thread");
});

test("a room's request goes after its thread's answer, failed or not", async () => {
  for (const settle of ["resolve", "reject"]) {
    const room = `room-${settle}`;
    let finish;
    const request = new Promise((resolve, reject) => {
      finish = settle === "resolve" ? resolve : reject;
    });
    noteThreadRequest(room, request);
    let released = false;
    const waiting = afterThreadRequest(room).then(() => {
      released = true;
    });
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(released, false);
    finish();
    await waiting;
    await afterThreadRequest(room);
  }
});

test("a thread that never answers does not hold its room", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  noteThreadRequest("stuck", new Promise(() => {}));
  const waiting = afterThreadRequest("stuck");
  t.mock.timers.tick(2_000);
  await waiting;
});
