import assert from "node:assert/strict";
import test from "node:test";

import {
  REMOTE_START_MAX_AGE_SECONDS,
  remoteStartTargets,
} from "./remoteStartCommand.ts";

const OWNER = "a".repeat(64);
const FIZZ = "1".repeat(64);
const HONEY = "2".repeat(64);
const NOW = 1_800_000_000;

function command(overrides = {}) {
  return {
    kind: 9,
    pubkey: OWNER,
    content: "!start",
    created_at: NOW - 5,
    tags: [
      ["h", "channel"],
      ["p", FIZZ.toUpperCase()],
    ],
    ...overrides,
  };
}

const local = (pubkey, status = "stopped", type = "local") => ({
  pubkey,
  status,
  backend: { type },
});

function targets(event, localAgents = [local(FIZZ), local(HONEY)]) {
  return remoteStartTargets(event, {
    localAgents,
    nowSeconds: NOW,
    ownerPubkey: OWNER,
  });
}

test("starts the stopped local agent the owner's !start mentions", () => {
  assert.deepEqual(targets(command()), [FIZZ]);
  assert.deepEqual(targets(command({ kind: 40002, content: " !start\n" })), [
    FIZZ,
  ]);
});

test("ignores other messages, senders, and stale commands", () => {
  assert.deepEqual(targets(command({ content: "!start please" })), []);
  assert.deepEqual(targets(command({ content: "!shutdown" })), []);
  assert.deepEqual(targets(command({ kind: 7 })), []);
  assert.deepEqual(targets(command({ pubkey: "b".repeat(64) })), []);
  assert.deepEqual(
    targets(command({ created_at: NOW - REMOTE_START_MAX_AGE_SECONDS - 1 })),
    [],
  );
});

test("leaves running, remote-deployed and other computers' agents alone", () => {
  assert.deepEqual(targets(command(), [local(FIZZ, "running")]), []);
  assert.deepEqual(
    targets(command(), [local(FIZZ, "not_deployed", "provider")]),
    [],
  );
  assert.deepEqual(targets(command(), [local(HONEY)]), []);
});
