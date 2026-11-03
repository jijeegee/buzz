import assert from "node:assert/strict";
import test from "node:test";

import {
  THREAD_NAME_MAX_PAGES,
  THREAD_NAME_PAGE_LIMIT,
  ThreadNameBatcher,
} from "./threadNameBatch.ts";

const flush = async () => {
  for (let i = 0; i < 20; i++) await Promise.resolve();
};

let nextId = 0;
const nameEvent = (room, head, content, created_at, id) => ({
  id: id ?? `n${String(nextId++).padStart(6, "0")}`,
  kind: 40009,
  pubkey: "b".repeat(64),
  content,
  created_at,
  tags: [
    ["h", room],
    ["e", head],
  ],
  sig: "",
});

/** A relay holding name events, answering newest first like the real one. */
function relay(events, { fail = false } = {}) {
  const reads = [];
  const fetchEvents = async (filter) => {
    reads.push(filter);
    if (fail) throw new Error("fixture relay failure");
    return events
      .filter(
        (event) =>
          filter.kinds.includes(event.kind) &&
          filter["#h"].includes(event.tags[0][1]) &&
          filter["#e"].includes(event.tags[1][1]) &&
          (filter.until === undefined || event.created_at <= filter.until),
      )
      .sort((a, b) =>
        a.created_at === b.created_at
          ? a.id.localeCompare(b.id)
          : b.created_at - a.created_at,
      )
      .slice(0, filter.limit);
  };
  return { reads, fetchEvents };
}

function harness(events, options) {
  const fake = relay(events, options);
  const timers = [];
  const covered = [];
  const batcher = new ThreadNameBatcher({
    fetchEvents: fake.fetchEvents,
    whenRoomCovered: async (roomId) => {
      covered.push(roomId);
    },
    setTimeout: (fn) => timers.push(fn),
  });
  const fire = async () => {
    await flush();
    for (const fn of timers.splice(0)) fn();
    await flush();
  };
  return { batcher, reads: fake.reads, covered, fire };
}

test("50 rows of one room share one read and get their newest names", async () => {
  const events = [];
  for (let i = 0; i < 50; i++) {
    events.push(nameEvent("a", `h${i}`, `old ${i}`, 10));
    events.push(nameEvent("a", `h${i}`, `new ${i}`, 20));
  }
  const { batcher, reads, covered, fire } = harness(events);
  const names = Array.from({ length: 50 }, (_, i) =>
    batcher.load("a", `h${i}`),
  );
  await fire();
  const resolved = await Promise.all(names);
  assert.equal(reads.length, 1);
  assert.deepEqual(reads[0].kinds, [40009]);
  assert.deepEqual(reads[0]["#h"], ["a"]);
  assert.equal(reads[0]["#e"].length, 50);
  assert.equal(reads[0].limit, THREAD_NAME_PAGE_LIMIT);
  assert.deepEqual(covered, ["a"], "the read waits for the room's coverage");
  for (const [i, event] of resolved.entries()) {
    assert.equal(event.content, `new ${i}`);
  }
});

test("rows in many rooms share reads of at most 100 heads", async () => {
  const { batcher, reads, fire } = harness([]);
  const names = [];
  for (let i = 0; i < 250; i++) names.push(batcher.load(`room${i}`, `h${i}`));
  await fire();
  assert.deepEqual(await Promise.all(names), Array(250).fill(null));
  assert.equal(reads.length, Math.ceil(250 / 100));
  for (const read of reads) {
    assert.ok(read["#e"].length <= 100);
    assert.ok(read["#h"].length <= 128);
  }
});

test("the same row asked twice is read once", async () => {
  const { batcher, reads, fire } = harness([nameEvent("a", "h", "name", 5)]);
  const first = batcher.load("a", "h");
  const second = batcher.load("a", "h");
  await fire();
  assert.equal((await first).content, "name");
  assert.equal((await second).content, "name");
  assert.deepEqual(reads[0]["#e"], ["h"]);
});

test("a name in another room for the same head is not used", async () => {
  const { batcher, fire } = harness([nameEvent("b", "h", "elsewhere", 5)]);
  const name = batcher.load("a", "h");
  await fire();
  assert.equal(await name, null);
});

test("pages drop named heads from #e and go back with an inclusive until", async () => {
  // Head "busy" has more renames than one page; "quiet" was named long ago.
  const events = [nameEvent("a", "quiet", "quiet name", 1)];
  for (let i = 0; i < THREAD_NAME_PAGE_LIMIT + 5; i++) {
    events.push(nameEvent("a", "busy", `busy ${i}`, 100 + i));
  }
  const { batcher, reads, fire } = harness(events);
  const busy = batcher.load("a", "busy");
  const quiet = batcher.load("a", "quiet");
  await fire();
  assert.equal((await busy).content, `busy ${THREAD_NAME_PAGE_LIMIT + 4}`);
  assert.equal((await quiet).content, "quiet name");
  assert.equal(reads.length, 2);
  assert.deepEqual(reads[1]["#e"], ["quiet"], "the named head left #e");
  assert.equal(reads[1].until, 105, "until is the oldest seen, inclusive");
});

test("the page cap leaves the rest unresolved instead of looping", async () => {
  const events = [];
  // Every page is full of another head's renames, one second each.
  for (
    let i = 0;
    i < THREAD_NAME_PAGE_LIMIT * (THREAD_NAME_MAX_PAGES + 2);
    i++
  ) {
    events.push(nameEvent("a", `noise${i % 99}`, "n", 100_000 - i));
  }
  const { batcher, reads, fire } = harness(events);
  const noise = Array.from({ length: 99 }, (_, i) =>
    batcher.load("a", `noise${i}`),
  );
  const missing = batcher.load("a", "never-named");
  const original = console.warn;
  console.warn = () => {};
  try {
    await fire();
    await Promise.all(noise);
    assert.equal(await missing, null);
  } finally {
    console.warn = original;
  }
  assert.ok(reads.length <= THREAD_NAME_MAX_PAGES);
});

test("a failed read rejects every caller in it", async () => {
  const { batcher, fire } = harness([], { fail: true });
  const a = batcher.load("a", "h1");
  const b = batcher.load("b", "h2");
  await fire();
  await assert.rejects(a, /fixture relay failure/);
  await assert.rejects(b, /fixture relay failure/);
});
