import assert from "node:assert/strict";
import test from "node:test";

import { INBOX_PRELOAD_ROWS } from "./inboxPreload.ts";
import {
  INBOX_PRELOAD_ROOMS_PER_REQUEST,
  runInboxPreload,
} from "./inboxPreloadRun.ts";

const room = (i) => ({ channelId: `room-${i}`, thread: null });
const thread = (i, root) => ({
  channelId: `room-${i}`,
  thread: { rootId: root, replyId: `${root}-reply` },
});

function harness() {
  const log = [];
  let release;
  const started = new Promise((resolve) => (release = resolve));
  return {
    log,
    release: () => release(true),
    deps: {
      waitStart: () => started,
      waitFirstThreads: async () => true,
      waitIdle: async () => {
        log.push("idle");
        return true;
      },
      preloadRooms: async (rooms, beforeFetch) => {
        if (beforeFetch) await beforeFetch();
        log.push(["rooms", rooms]);
      },
      preloadThreads: async (threads, beforeRequest) => {
        await beforeRequest(threads);
        log.push(["threads", threads.map((t) => t.rootId)]);
      },
    },
  };
}

test("the first wave waits for startup idle before any request", async () => {
  const { log, release, deps } = harness();
  const run = runInboxPreload([room(0), room(1)], deps);
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.deepEqual(log, []);
  release();
  await run;
  assert.deepEqual(log, [["rooms", ["room-0", "room-1"]]]);
});

test("rows 13 to 48 load in a second wave, twelve rooms per idle request", async () => {
  const { log, release, deps } = harness();
  const targets = [
    ...Array.from({ length: 40 }, (_, i) => room(i)),
    // A thread row of a first-wave room loads its thread only.
    thread(3, "t-late"),
  ];
  targets.splice(2, 0, thread(1, "t-first"));
  release();
  await runInboxPreload(targets, deps);
  const firstRooms = targets
    .slice(0, INBOX_PRELOAD_ROWS)
    .map((target) => target.channelId);
  const firstUnique = [...new Set(firstRooms)];
  assert.deepEqual(log[0], ["rooms", firstUnique]);
  assert.deepEqual(log[1], ["threads", ["t-first"]]);
  const rest = log.slice(2);
  // Each background request waits for idle first.
  const requests = rest.filter((entry) => entry !== "idle");
  const idles = rest.filter((entry) => entry === "idle");
  const roomRequests = requests.filter((entry) => entry[0] === "rooms");
  assert.ok(
    roomRequests.every(
      ([, rooms]) => rooms.length <= INBOX_PRELOAD_ROOMS_PER_REQUEST,
    ),
  );
  const waveTwo = roomRequests.flatMap(([, rooms]) => rooms);
  assert.equal(new Set([...firstUnique, ...waveTwo]).size, 40);
  assert.ok(waveTwo.every((roomId) => !firstUnique.includes(roomId)));
  assert.equal(roomRequests.length, Math.ceil(waveTwo.length / 12));
  // Each wave-two request re-checks idle after its store check too.
  const roomIdles = rest.filter(
    (entry, i) => entry === "idle" && rest[i + 1] === "idle",
  );
  assert.equal(roomIdles.length, roomRequests.length);
  assert.ok(idles.length >= roomRequests.length + 1);
  assert.deepEqual(requests.at(-1), ["threads", ["t-late"]]);
});

test("a busy pass that is cancelled stops before the next wave request", async () => {
  const { log, release, deps } = harness();
  deps.waitIdle = async () => false;
  release();
  await runInboxPreload(
    Array.from({ length: 30 }, (_, i) => room(i)),
    deps,
  );
  assert.equal(log.length, 1);
  assert.equal(log[0][0], "rooms");
});
