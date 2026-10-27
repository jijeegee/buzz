import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  MessageStoreCapture,
  MESSAGE_STORE_KINDS,
} from "./messageStoreCapture.ts";

const fixture = JSON.parse(
  readFileSync(
    new URL("../../../../test-fixtures/message-store.json", import.meta.url),
  ),
);

test("shared stored kinds exclude transport metadata and agent frames", () => {
  assert.deepEqual([...MESSAGE_STORE_KINDS], fixture.storedKinds);
  for (const kind of fixture.excludedKinds)
    assert.equal(MESSAGE_STORE_KINDS.has(kind), false);
});

test("capture receives a reply without waiting for disk, retains its original lease", async () => {
  const calls = [];
  let complete;
  const write = new Promise((resolve) => {
    complete = resolve;
  });
  const lease = { relay: "https://one", account: "a", epoch: 0 };
  const capture = new MessageStoreCapture(async (command, args) => {
    calls.push([command, args]);
    return command === "message_store_begin" ? lease : write;
  });
  capture.receive(fixture.events[1]);
  for (let i = 0; i < 5; i++) await Promise.resolve();
  assert.deepEqual(calls[1], [
    "message_store_put",
    { lease, events: [fixture.events[1]] },
  ]);
  capture.close();
  capture.receive(fixture.events[3]);
  complete();
  await capture.flush();
  assert.equal(calls.length, 2);
});

test("disk failure is reported and a subsequent batch can be written", async () => {
  const errors = [];
  let fail = true;
  let writes = 0;
  const capture = new MessageStoreCapture(
    async (command) => {
      if (command === "message_store_begin")
        return { relay: "https://one", account: "a", epoch: 0 };
      writes++;
      if (fail) throw new Error("disk full");
    },
    (error) => errors.push(error),
  );
  capture.receive(fixture.events[0]);
  await capture.flush();
  fail = false;
  capture.receive(fixture.events[1]);
  await capture.flush();
  assert.equal(errors.length, 1);
  assert.equal(writes, 2);
});
