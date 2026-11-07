import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";

import { QueryClient } from "@tanstack/react-query";

import { setStoreScope } from "./paintCoverage.ts";
import {
  checkedWindow,
  resetWindowCheckGuards,
  setWindowCheckDeps,
  WINDOW_CHECK_GUARD_MS,
} from "./windowCheck.ts";
import { roomPageRows, threadPageRows } from "./windowCheckIo.ts";
import {
  noteServerWindowCheckMode,
  resetWindowCheckModes,
  WINDOW_CHECK_REASK_MS,
} from "./windowCheckMode.ts";

const BR = String.fromCharCode(10);
const RELAY = "wss://relay.test";
const key = { roomId: "room", rootId: null, cursor: null, limit: 3 };

const row = (id, createdAt) => ({ id, kind: 9, created_at: createdAt });
const rows = [row("c", 30), row("b", 20), row("a", 10)];

function storedWindow(overrides = {}) {
  return {
    events: rows,
    count: 3,
    oldest: { createdAt: 10, eventId: "a" },
    hasMore: true,
    fullyStored: true,
    ...overrides,
  };
}

/** A fake relay and store; `stores` are read one after another. */
function setup({ stores, checks = [], live = () => 0, now = () => 1_000 }) {
  const calls = {
    stored: 0,
    check: 0,
    replace: [],
    release: [],
    heads: [],
    log: [],
  };
  const restore = setWindowCheckDeps({
    stored: async () => {
      const next = stores[Math.min(calls.stored, stores.length - 1)];
      calls.stored += 1;
      return next;
    },
    check: async () => {
      const next = checks[Math.min(calls.check, checks.length - 1)];
      calls.check += 1;
      if (next instanceof Error) throw next;
      return next && "page" in next
        ? next
        : { answer: next ?? null, page: null };
    },
    withHeads: async (roomIds, request, answers) => {
      const answer = await request();
      calls.heads.push(...answers(answer).map((head) => ({ roomIds, head })));
      return answer;
    },
    mark: async () => "mark",
    release: async (mark) => {
      calls.release.push(mark);
    },
    replace: async (mark, windowKey, oldest, ids) => {
      calls.replace.push({ mark, windowKey, oldest, ids });
    },
    liveActivity: live,
    nowMs: now,
    log: (line) => calls.log.push(line),
  });
  return { calls, restore };
}

function io(page = { ids: ["c", "b", "a", "x"], rows, hasMore: true }) {
  const fetches = [];
  return {
    fetches,
    io: {
      fetch: async (includeAux) => {
        fetches.push(includeAux);
        return "relay";
      },
      page: () => page,
      fromStore: (stored) => stored.events,
    },
  };
}

function client() {
  const queryClient = new QueryClient();
  setStoreScope(queryClient, { pubkey: "me", relayUrl: RELAY });
  return queryClient;
}

let restore = () => {};
beforeEach(() => {
  resetWindowCheckModes();
  resetWindowCheckGuards();
  noteServerWindowCheckMode(RELAY, "on", 1_000);
});
afterEach(() => restore());

const equalAnswer = { oldestId: "a", count: 3, mode: "on" };

test("an equal check opens the stored window without a page", async () => {
  const fake = setup({ stores: [storedWindow()], checks: [equalAnswer] });
  restore = fake.restore;
  const page = io();
  const answer = await checkedWindow(client(), key, page.io);
  assert.equal(answer, rows);
  assert.deepEqual(page.fetches, []);
  assert.equal(fake.calls.check, 1);
});

test("an equal check records what its page would have", async () => {
  const fake = setup({ stores: [storedWindow()], checks: [equalAnswer] });
  restore = fake.restore;
  const page = io();
  page.io.head = (events) => ({ roomId: "room", events });
  await checkedWindow(client(), key, page.io);
  // The page span, under the mark taken before the check.
  assert.deepEqual(fake.calls.replace, [
    {
      mark: "mark",
      windowKey: key,
      oldest: { createdAt: 10, eventId: "a" },
      ids: ["c", "b", "a"],
    },
  ]);
  // The head span a newest window's request records.
  assert.deepEqual(fake.calls.heads, [
    { roomIds: ["room"], head: { roomId: "room", events: rows } },
  ]);
});

test("a different check records no head of its own and frees its mark", async () => {
  const fake = setup({
    stores: [storedWindow({ count: 2 }), storedWindow()],
    checks: [equalAnswer],
  });
  restore = fake.restore;
  const page = io();
  page.io.head = (events) => ({ roomId: "room", events });
  await checkedWindow(client(), key, page.io);
  assert.deepEqual(fake.calls.heads, []);
  assert.deepEqual(fake.calls.release, ["mark"]);
  assert.equal(fake.calls.replace.length, 1, "only the refetched page");
});

test("a row missing from the store refetches and replaces its stretch", async () => {
  // The store lacks `b`: its newest 3 reach down to an older row.
  const fake = setup({
    stores: [
      storedWindow({ oldest: { createdAt: 5, eventId: "z" } }),
      storedWindow(),
    ],
    checks: [equalAnswer],
  });
  restore = fake.restore;
  const page = io();
  assert.equal(await checkedWindow(client(), key, page.io), "relay");
  assert.deepEqual(page.fetches, [true]);
  assert.deepEqual(fake.calls.replace, [
    {
      mark: "mark",
      windowKey: key,
      oldest: { createdAt: 10, eventId: "a" },
      ids: ["c", "b", "a", "x"],
    },
  ]);
  // The store agrees with the page now: no guard, the next open checks.
  const again = setup({ stores: [storedWindow()], checks: [equalAnswer] });
  restore = again.restore;
  await checkedWindow(client(), key, io().io);
  assert.equal(again.calls.check, 1);
});

test("a deleted row left in the store refetches", async () => {
  // The store still holds `d`, deleted on the relay: its window ends at b.
  const fake = setup({
    stores: [
      storedWindow({ oldest: { createdAt: 20, eventId: "b" } }),
      storedWindow(),
    ],
    checks: [equalAnswer],
  });
  restore = fake.restore;
  const page = io();
  await checkedWindow(client(), key, page.io);
  assert.deepEqual(page.fetches, [true]);
  assert.equal(fake.calls.replace.length, 1);
});

test("a short window whose count differs refetches", async () => {
  const fake = setup({
    stores: [
      storedWindow({ count: 2, hasMore: false }),
      storedWindow({ count: 3, hasMore: false }),
    ],
    checks: [{ oldestId: "a", count: 3, mode: "on" }],
  });
  restore = fake.restore;
  const page = io({ ids: ["c", "b", "a"], rows, hasMore: false });
  await checkedWindow(client(), { ...key, limit: 5 }, page.io);
  assert.deepEqual(page.fetches, [true]);
  // A last page opens the lower bound.
  assert.equal(fake.calls.replace[0].oldest, null);
});

test("ids compare without regard to case", async () => {
  const fake = setup({
    stores: [storedWindow()],
    checks: [{ oldestId: "A", count: 3, mode: "on" }],
  });
  restore = fake.restore;
  const page = io();
  await checkedWindow(client(), key, page.io);
  assert.deepEqual(page.fetches, []);
});

test("a window still different after its refetch takes today's path for a while", async () => {
  let now = 1_000;
  const differing = storedWindow({ oldest: { createdAt: 5, eventId: "z" } });
  const fake = setup({
    stores: [differing],
    checks: [equalAnswer],
    now: () => now,
  });
  restore = fake.restore;
  await checkedWindow(client(), key, io().io);
  assert.equal(fake.calls.replace.length, 1);
  assert.match(fake.calls.log.join("\n"), /still differs after a refetch/);

  now += WINDOW_CHECK_GUARD_MS - 1;
  const page = io();
  await checkedWindow(client(), key, page.io);
  assert.deepEqual(page.fetches, [false], "today's request");
  assert.equal(fake.calls.check, 1, "no check while guarded");

  // Other rooms are not guarded.
  await checkedWindow(client(), { ...key, roomId: "other" }, io().io);
  assert.equal(fake.calls.check, 2);

  now += 1;
  noteServerWindowCheckMode(RELAY, "on", now);
  await checkedWindow(client(), key, io().io);
  assert.equal(fake.calls.check, 3, "checked again after the guard");
});

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

test("shadow fetches alongside the check, never after it", async () => {
  noteServerWindowCheckMode(RELAY, "shadow", 1_000);
  const fake = setup({ stores: [storedWindow()] });
  restore = fake.restore;
  let answerCheck = () => {};
  const restoreCheck = setWindowCheckDeps({
    check: () =>
      new Promise((resolve) => {
        answerCheck = () =>
          resolve({ answer: { ...equalAnswer, mode: "shadow" }, page: null });
      }),
  });
  const page = io();
  // The check never answers before the page: the open does not wait on it.
  assert.equal(await checkedWindow(client(), key, page.io), "relay");
  assert.deepEqual(page.fetches, [true]);
  assert.equal(fake.calls.replace.length, 1, "the page is kept");
  answerCheck();
  await tick();
  restoreCheck();
  assert.match(fake.calls.log.join(BR), /shadow room room equal, page agrees/);
});

test("shadow logs a different window and keeps the page", async () => {
  noteServerWindowCheckMode(RELAY, "shadow", 1_000);
  const fake = setup({
    stores: [storedWindow({ count: 2 }), storedWindow()],
    checks: [{ ...equalAnswer, mode: "shadow" }],
  });
  restore = fake.restore;
  const page = io();
  await checkedWindow(client(), key, page.io);
  await tick();
  assert.deepEqual(page.fetches, [true]);
  assert.equal(fake.calls.replace.length, 1);
  assert.match(fake.calls.log.join(BR), /shadow room room differ/);
});

test("a first check that says shadow fetches today's page", async () => {
  resetWindowCheckModes();
  const fake = setup({
    stores: [storedWindow()],
    checks: [{ ...equalAnswer, mode: "shadow" }],
  });
  restore = fake.restore;
  const page = io();
  assert.equal(await checkedWindow(client(), key, page.io), "relay");
  assert.deepEqual(page.fetches, [false]);
  assert.deepEqual(fake.calls.replace, []);
});

test("a thread check that fails is today's path, without a word", async () => {
  const threadKey = { ...key, rootId: "root" };
  const fake = setup({
    stores: [storedWindow()],
    checks: [new Error("unknown filter field")],
  });
  restore = fake.restore;
  const page = io();
  assert.equal(await checkedWindow(client(), threadKey, page.io), "relay");
  assert.deepEqual(page.fetches, [false]);
  assert.deepEqual(fake.calls.log, []);
  assert.deepEqual(fake.calls.release, ["mark"]);
  // Counted as off: the next open asks nothing.
  const next = io();
  await checkedWindow(client(), threadKey, next.io);
  assert.deepEqual(next.fetches, [false]);
  assert.equal(fake.calls.check, 1);
});

test("an old relay's page answer to a room check is the page", async () => {
  resetWindowCheckModes();
  let now = 1_000;
  const oldPage = [row("c", 30), { id: "bounds", kind: 39006 }];
  const fake = setup({
    stores: [storedWindow()],
    checks: [{ answer: null, page: oldPage }],
    now: () => now,
  });
  restore = fake.restore;
  const first = io();
  first.io.fromCheckPage = (events) => events;
  first.io.head = (events) => ({ roomId: "room", events });
  assert.equal(await checkedWindow(client(), key, first.io), oldPage);
  assert.deepEqual(first.fetches, [], "one request, not two");
  assert.deepEqual(fake.calls.heads, [
    { roomIds: ["room"], head: { roomId: "room", events: oldPage } },
  ]);

  // Within ten minutes: today's request only.
  now += WINDOW_CHECK_REASK_MS - 1;
  const second = io();
  await checkedWindow(client(), key, second.io);
  assert.deepEqual(second.fetches, [false]);
  assert.equal(fake.calls.check, 1);

  // Then asked again, once.
  now += 1;
  const third = io();
  third.io.fromCheckPage = (events) => events;
  await checkedWindow(client(), key, third.io);
  assert.equal(fake.calls.check, 2);
  assert.deepEqual(third.fetches, []);
});

test("off is today's path, asked again after ten minutes", async () => {
  let now = 1_000;
  const fake = setup({
    stores: [storedWindow()],
    checks: [{ ...equalAnswer, mode: "off" }],
    now: () => now,
  });
  restore = fake.restore;
  const first = io();
  assert.equal(await checkedWindow(client(), key, first.io), "relay");
  assert.deepEqual(first.fetches, [false]);
  assert.equal(fake.calls.check, 1);

  now += WINDOW_CHECK_REASK_MS - 1;
  const second = io();
  await checkedWindow(client(), key, second.io);
  assert.deepEqual(second.fetches, [false]);
  assert.equal(fake.calls.check, 1, "no check while off");
  assert.equal(fake.calls.stored, 1, "no store read while off");
  assert.deepEqual(fake.calls.replace, []);

  now += 1;
  await checkedWindow(client(), key, io().io);
  assert.equal(fake.calls.check, 2);
});

test("a relay without the check counts as off", async () => {
  const fake = setup({ stores: [storedWindow()], checks: [null] });
  restore = fake.restore;
  const page = io();
  await checkedWindow(client(), key, page.io);
  assert.deepEqual(page.fetches, [false]);
  const next = io();
  await checkedWindow(client(), key, next.io);
  assert.equal(fake.calls.check, 1);
});

test("the setting overrides the relay's mode", async () => {
  resetWindowCheckModes("off");
  const fake = setup({ stores: [storedWindow()], checks: [equalAnswer] });
  restore = fake.restore;
  const page = io();
  await checkedWindow(client(), key, page.io);
  assert.deepEqual(page.fetches, [false]);
  assert.equal(fake.calls.check, 0);

  resetWindowCheckModes("on");
  noteServerWindowCheckMode(RELAY, "off", 1_000);
  const on = io();
  await checkedWindow(client(), key, on.io);
  assert.deepEqual(on.fetches, [], "on despite the relay's off");
});

test("live rows arriving during the check compare once more", async () => {
  let live = 0;
  const fake = setup({
    stores: [storedWindow({ count: 2 }), storedWindow()],
    checks: [equalAnswer],
    live: () => live,
  });
  restore = fake.restore;
  const restoreCheck = setWindowCheckDeps({
    check: async () => {
      fake.calls.check += 1;
      live += 1;
      return { answer: equalAnswer, page: null };
    },
  });
  const page = io();
  await checkedWindow(client(), key, page.io);
  restoreCheck();
  assert.equal(fake.calls.check, 2);
  assert.deepEqual(page.fetches, [], "equal on the second look");
});

test("a window the store does not hold is fetched and kept, unchecked", async () => {
  const fake = setup({
    stores: [storedWindow({ fullyStored: false })],
    checks: [equalAnswer],
  });
  restore = fake.restore;
  const page = io();
  await checkedWindow(client(), key, page.io);
  assert.equal(fake.calls.check, 0);
  assert.deepEqual(page.fetches, [true]);
  assert.equal(fake.calls.replace.length, 1);
});

test("the first open of a run fetches as today and learns the mode", async () => {
  resetWindowCheckModes();
  const fake = setup({
    stores: [storedWindow({ fullyStored: false })],
    checks: [equalAnswer],
  });
  restore = fake.restore;
  const page = io();
  await checkedWindow(client(), key, page.io);
  assert.deepEqual(page.fetches, [false]);
  assert.deepEqual(fake.calls.replace, []);
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(fake.calls.check, 1, "the first check is always sent");
  const next = io();
  await checkedWindow(client(), key, next.io);
  assert.deepEqual(next.fetches, [true], "now on: kept as a page");
});

test("a fully stored first open checks at once", async () => {
  resetWindowCheckModes();
  const fake = setup({ stores: [storedWindow()], checks: [equalAnswer] });
  restore = fake.restore;
  const page = io();
  await checkedWindow(client(), key, page.io);
  assert.deepEqual(page.fetches, []);
});

test("no window in the answer keeps nothing and releases the mark", async () => {
  const fake = setup({
    stores: [storedWindow({ fullyStored: false })],
    checks: [equalAnswer],
  });
  restore = fake.restore;
  const page = io(null);
  await checkedWindow(client(), key, page.io);
  assert.deepEqual(fake.calls.replace, []);
  assert.deepEqual(fake.calls.release, ["mark"]);
});

test("a failed fetch releases the mark", async () => {
  const fake = setup({
    stores: [storedWindow({ fullyStored: false })],
    checks: [equalAnswer],
  });
  restore = fake.restore;
  await assert.rejects(
    checkedWindow(client(), key, {
      fetch: async () => {
        throw new Error("offline");
      },
      page: () => null,
      fromStore: () => null,
    }),
  );
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(fake.calls.release, ["mark"]);
});

test("a cache without an account and server is today's path", async () => {
  const fake = setup({ stores: [storedWindow()], checks: [equalAnswer] });
  restore = fake.restore;
  const page = io();
  await checkedWindow(new QueryClient(), key, page.io);
  assert.deepEqual(page.fetches, [false]);
  assert.equal(fake.calls.stored, 0);
});

test("a room page's rows are its window kinds, with has_more from its bounds", () => {
  const bounds = {
    id: "bounds",
    kind: 39006,
    created_at: 0,
    content: JSON.stringify({ has_more: false, next_cursor: null }),
  };
  const page = roomPageRows([
    row("c", 30),
    { id: "huddle-ended", kind: 48103, created_at: 25 },
    { id: "reaction", kind: 7, created_at: 31 },
    { id: "summary", kind: 39005, created_at: 31 },
    bounds,
  ]);
  assert.deepEqual(
    page.rows.map((event) => event.id),
    ["c", "huddle-ended"],
  );
  assert.deepEqual(page.ids, ["c", "huddle-ended", "reaction", "summary"]);
  assert.equal(page.hasMore, false);
  assert.equal(roomPageRows([row("c", 30)]), null, "no bounds");
});

test("a thread page's rows are its replies, never the root", () => {
  const page = threadPageRows("root", {
    channelId: "room",
    rootId: "root",
    events: [
      row("root", 1),
      row("r1", 10),
      { id: "edit", kind: 40003, created_at: 11 },
    ],
    hasMore: false,
    nextCursor: null,
  });
  assert.deepEqual(
    page.rows.map((event) => event.id),
    ["r1"],
  );
  assert.equal(threadPageRows("root", null), null);
});
