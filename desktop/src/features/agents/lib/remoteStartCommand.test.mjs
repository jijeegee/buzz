import assert from "node:assert/strict";
import test from "node:test";

import {
  agentDmChannelIds,
  REMOTE_START_MAX_AGE_SECONDS,
  RemoteCommandLedger,
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
  assert.deepEqual(targets(command({ content: " !start\n" })), [FIZZ]);
  // Only kind:9, which the agent's harness also consumes.
  assert.deepEqual(targets(command({ kind: 40002 })), []);
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

test("leaves another account's agent on this computer alone", () => {
  const owned = { ...local(FIZZ), ownerPubkey: OWNER };
  const other = { ...local(FIZZ), ownerPubkey: "b".repeat(64) };
  assert.deepEqual(targets(command(), [owned]), [FIZZ]);
  assert.deepEqual(targets(command(), [other]), []);
});

test("acts on each command once and only while it is the latest", () => {
  const ledger = new RemoteCommandLedger();
  const start = command({ id: "s1", created_at: NOW - 10 });
  assert.equal(ledger.record(start), true);
  assert.equal(ledger.record(start), false, "a replay is not new");
  assert.equal(ledger.isLatest(start, FIZZ), true);

  // A later Stop, even replayed out of order, cancels the Start.
  ledger.record(
    command({ id: "x1", content: "!shutdown", created_at: NOW - 5 }),
  );
  assert.equal(ledger.isLatest(start, FIZZ), false);

  // An older Stop seen afterwards does not.
  const again = command({ id: "s2", created_at: NOW - 3 });
  ledger.record(again);
  ledger.record(
    command({ id: "x0", content: "!shutdown", created_at: NOW - 20 }),
  );
  assert.equal(ledger.isLatest(again, FIZZ), true);

  // Other messages never count as commands.
  ledger.record(command({ id: "m1", content: "hello", created_at: NOW }));
  assert.equal(ledger.isLatest(again, FIZZ), true);
});

test("finds the owner's two-person DMs with this computer's agents", () => {
  const dm = (id, participantPubkeys, channelType = "dm") => ({
    id,
    channelType,
    participantPubkeys,
  });
  const channels = [
    dm("fizz-dm", [OWNER, FIZZ.toUpperCase()]),
    dm("group-dm", [OWNER, FIZZ, HONEY]),
    dm("someone-else", ["b".repeat(64), FIZZ]),
    dm("stream", [OWNER, FIZZ], "stream"),
    dm("unknown-agent", [OWNER, "3".repeat(64)]),
  ];
  assert.deepEqual(agentDmChannelIds(channels, [FIZZ, HONEY], OWNER), [
    "fizz-dm",
  ]);
});
