import assert from "node:assert/strict";
import { test } from "node:test";

import { afterThreadRequest, noteThreadRequest } from "./threadFirst.ts";

const client = {};

test("a room with no thread opening goes at once", async () => {
  await afterThreadRequest(client, "no-thread");
});

test("a room's request goes after its thread's answer, failed or not", async () => {
  for (const settle of ["resolve", "reject"]) {
    const room = `room-${settle}`;
    let finish;
    const request = new Promise((resolve, reject) => {
      finish = settle === "resolve" ? resolve : reject;
    });
    noteThreadRequest(client, room, request);
    let released = false;
    const waiting = afterThreadRequest(client, room).then(() => {
      released = true;
    });
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(released, false);
    finish();
    await waiting;
    await afterThreadRequest(client, room);
  }
});

test("a thread that never answers does not hold its room", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  noteThreadRequest(client, "stuck", new Promise(() => {}));
  const waiting = afterThreadRequest(client, "stuck");
  t.mock.timers.tick(2_000);
  await waiting;
});

test("another account's thread does not hold this room", async () => {
  noteThreadRequest({}, "shared", new Promise(() => {}));
  await afterThreadRequest(client, "shared");
});
