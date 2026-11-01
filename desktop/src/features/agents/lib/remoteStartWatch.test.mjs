import assert from "node:assert/strict";
import { afterEach, mock, test } from "node:test";

import {
  REMOTE_START_COVERAGE_RETRIES,
  startRemoteStartWatch,
} from "./remoteStartWatch.ts";
import { RemoteCommandLedger } from "./remoteStartCommand.ts";

const OWNER = "a".repeat(64);
const AGENT = "c".repeat(64);
const OTHER = "d".repeat(64);

const flush = async () => {
  for (let i = 0; i < 20; i++) await Promise.resolve();
};

function command(id, roomId, content = "!start", overrides = {}) {
  return {
    id,
    kind: 9,
    pubkey: OWNER,
    content,
    created_at: 1_000,
    tags: [
      ["h", roomId],
      ["p", AGENT],
    ],
    sig: "",
    ...overrides,
  };
}

/** The watch over a fake bundle; starts are counted through a real ledger. */
function harness({ inHuddleWindow = false, agentDms = ["dm-agent"] } = {}) {
  const all = new Set();
  const quiet = new Set();
  const reads = [];
  const coverage = [];
  const ledger = new RemoteCommandLedger();
  const starts = [];
  let relayEvents = [];
  const watch = startRemoteStartWatch({
    ownerPubkey: OWNER,
    inHuddleWindow,
    listenAllRooms: (listener) => {
      all.add(listener);
      return () => all.delete(listener);
    },
    listenQuietRooms: (listener) => {
      quiet.add(listener);
      return () => quiet.delete(listener);
    },
    whenRoomsCovered: (roomIds) =>
      new Promise((resolve) => coverage.push({ roomIds, resolve })),
    fetchEvents: async (filter) => {
      reads.push(filter);
      return relayEvents.filter((event) =>
        filter["#h"].includes(event.tags[0][1]),
      );
    },
    isAgentDm: async (roomId) => agentDms.includes(roomId),
    handle: (event) => {
      if (!ledger.record(event)) return;
      if (event.content === "!start") starts.push(event.id);
    },
    nowSeconds: () => 1_100,
  });
  return {
    watch,
    reads,
    coverage,
    starts,
    listeners: () => all.size + quiet.size,
    setRelayEvents: (events) => {
      relayEvents = events;
    },
    live: (event) => {
      for (const listener of [...all]) listener(event);
    },
    quiet: async (event) => {
      for (const listener of [...quiet]) listener(event);
      await flush();
    },
  };
}

afterEach(() => mock.timers.reset());

test("the same !start live, from repair and from the read-back starts once", async () => {
  const h = harness();
  h.watch.setWanted(["dm-agent"]);
  h.watch.setWanted(["dm-agent", "dm-new"]);
  const start = command("start-1", "dm-new");
  h.live(start);
  // Reconnect repair delivers it again on the same listener.
  h.live(start);
  h.setRelayEvents([start]);
  h.coverage[0].resolve(true);
  await flush();
  assert.equal(h.reads.length, 1);
  assert.deepEqual(h.starts, ["start-1"]);
});

test("a start sent during a drop comes back through repair once", () => {
  const h = harness();
  h.watch.setWanted(["dm-agent"]);
  const start = command("during-drop", "dm-agent");
  h.live(start);
  h.live(start);
  assert.deepEqual(h.starts, ["during-drop"]);
});

test("the first set reads nothing back; an added DM reads once after coverage", async () => {
  const h = harness();
  h.watch.setWanted([]);
  h.watch.setWanted(["dm-agent"]);
  await flush();
  assert.equal(h.coverage.length, 0, "the baseline reads nothing");
  assert.equal(h.reads.length, 0);
  h.watch.setWanted(["dm-agent", "dm-new"]);
  await flush();
  assert.deepEqual(h.coverage[0].roomIds, ["dm-new"]);
  assert.equal(h.reads.length, 0, "waits for the chunk first");
  h.coverage[0].resolve(true);
  await flush();
  assert.equal(h.reads.length, 1);
  assert.deepEqual(h.reads[0], {
    kinds: [9],
    authors: [OWNER],
    "#h": ["dm-new"],
    since: 1_100 - 120,
    limit: 20,
  });
});

test("late coverage: the chunk goes live at 3 s, a !start sent at 2 s starts once", async () => {
  const h = harness();
  h.watch.setWanted(["dm-agent"]);
  h.watch.setWanted(["dm-agent", "dm-new"]);
  await flush();
  // 1.5 s: the bound passes before the chunk is live; nothing sent yet.
  h.coverage[0].resolve(false);
  await flush();
  assert.equal(h.reads.length, 1);
  assert.deepEqual(h.starts, []);
  // 2 s: the command is sent, but no chunk carries the DM yet.
  const start = command("start-late", "dm-new");
  h.setRelayEvents([start]);
  // 3 s: covered. Read once more.
  h.coverage[1].resolve(true);
  await flush();
  assert.equal(h.reads.length, 2);
  assert.deepEqual(h.starts, ["start-late"]);
  // Then live delivery of the same event changes nothing.
  h.live(start);
  assert.deepEqual(h.starts, ["start-late"]);
});

test("coverage that never comes is waited for a bounded number of times", async () => {
  const h = harness();
  h.watch.setWanted(["dm-agent"]);
  h.watch.setWanted(["dm-agent", "dm-new"]);
  for (let i = 0; i <= REMOTE_START_COVERAGE_RETRIES + 2; i++) {
    await flush();
    h.coverage[i]?.resolve(false);
  }
  await flush();
  assert.equal(h.coverage.length, 1 + REMOTE_START_COVERAGE_RETRIES);
  assert.equal(h.reads.length, 1);
});

test("a hidden agent DM's !start starts; a hidden group DM's does not", async () => {
  const h = harness({ agentDms: ["hidden-agent-dm"] });
  h.watch.setWanted(["dm-agent"]);
  await h.quiet(command("in-group", "hidden-group"));
  await h.quiet(command("in-agent-dm", "hidden-agent-dm"));
  assert.deepEqual(h.starts, ["in-agent-dm"]);
});

test("other rooms, other authors and other kinds are ignored", () => {
  const h = harness();
  h.watch.setWanted(["dm-agent"]);
  h.live(command("elsewhere", "general"));
  h.live(command("not-owner", "dm-agent", "!start", { pubkey: OTHER }));
  h.live(command("chat", "dm-agent", "hello"));
  h.live(command("reaction", "dm-agent", "!start", { kind: 7 }));
  assert.deepEqual(h.starts, []);
});

test("stop detaches every listener and drops pending reads", async () => {
  const h = harness();
  h.watch.setWanted(["dm-agent"]);
  h.watch.setWanted(["dm-agent", "dm-new"]);
  await flush();
  h.watch.stop();
  assert.equal(h.listeners(), 0);
  h.setRelayEvents([command("after-stop", "dm-new")]);
  h.coverage[0].resolve(true);
  await flush();
  assert.equal(h.reads.length, 0);
  assert.deepEqual(h.starts, []);
});

test("the huddle window attaches nothing", async () => {
  const h = harness({ inHuddleWindow: true });
  h.watch.setWanted(["dm-agent"]);
  h.watch.setWanted(["dm-agent", "dm-new"]);
  await flush();
  assert.equal(h.listeners(), 0);
  assert.equal(h.coverage.length, 0);
});
