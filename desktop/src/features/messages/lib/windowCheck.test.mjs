import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";

import { QueryClient } from "@tanstack/react-query";

import { setStoreScope } from "./paintCoverage.ts";
import {
  checkedWindow,
  hexIdBefore,
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
function setup({
  stores,
  checks = [],
  live = () => 0,
  now = () => 1_000,
  applied = true,
}) {
  const calls = {
    stored: 0,
    storedKeys: [],
    check: 0,
    replace: [],
    release: [],
    heads: [],
    log: [],
    order: [],
  };
  const restore = setWindowCheckDeps({
    stored: async (_scope, windowKey) => {
      const next = stores[Math.min(calls.stored, stores.length - 1)];
      calls.stored += 1;
      calls.storedKeys.push(windowKey);
      calls.order.push("stored");
      return next;
    },
    check: async () => {
      const next = checks[Math.min(calls.check, checks.length - 1)];
      calls.check += 1;
      calls.order.push("check");
      if (next instanceof Error) throw next;
      // null: a relay without the check (an answer with no check in it).
      if (next === null) return { answer: null, noCheck: true, page: null };
      return next && "page" in next
        ? { noCheck: false, ...next }
        : { answer: next ?? null, noCheck: false, page: null };
    },
    withHeads: async (roomIds, request, answers) => {
      const answer = await request();
      calls.heads.push(...answers(answer).map((head) => ({ roomIds, head })));
      return answer;
    },
    mark: async () => {
      calls.order.push("mark");
      return "mark";
    },
    release: async (mark) => {
      calls.release.push(mark);
    },
    replace: async (mark, scope, windowKey, oldest, ids) => {
      assert.deepEqual(scope, { pubkey: "me", relayUrl: RELAY });
      calls.replace.push({ mark, windowKey, oldest, ids });
      return applied;
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
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

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
  await tick();
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
  await tick();
  assert.match(fake.calls.log.join(BR), /still differs after a refetch/);
  // Only rows at or below the page's newest (c at 30) are compared.
  assert.deepEqual(fake.calls.storedKeys.at(-1).cursor, {
    createdAt: 30,
    eventId: "b",
  });

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

test("a refetch the store did not apply guards nothing", async () => {
  const differing = storedWindow({ oldest: { createdAt: 5, eventId: "z" } });
  const fake = setup({
    stores: [differing],
    checks: [equalAnswer],
    applied: false,
  });
  restore = fake.restore;
  await checkedWindow(client(), key, io().io);
  await tick();
  assert.equal(fake.calls.stored, 1, "no re-read");
  assert.deepEqual(
    fake.calls.log.filter((line) => /still/.test(line)),
    [],
  );
});

test("a store that cannot be re-read after a refetch is not a difference", async () => {
  const differing = storedWindow({ oldest: { createdAt: 5, eventId: "z" } });
  const fake = setup({ stores: [differing, null], checks: [equalAnswer] });
  restore = fake.restore;
  await checkedWindow(client(), key, io().io);
  await tick();
  assert.equal(fake.calls.stored, 2);
  assert.deepEqual(
    fake.calls.log.filter((line) => /still/.test(line)),
    [],
  );
  await checkedWindow(client(), key, io().io);
  assert.equal(fake.calls.check, 2, "not guarded");
});

test("hexIdBefore is the id just before, same length", () => {
  assert.equal(hexIdBefore("c"), "b");
  assert.equal(hexIdBefore("a0"), "9f");
  assert.equal(hexIdBefore("B00"), "aff");
  assert.equal(hexIdBefore("000"), null);
  assert.equal(hexIdBefore("xyz"), null);
});

test("the check goes out before the store is read, the mark before the read", async () => {
  const fake = setup({ stores: [storedWindow()], checks: [equalAnswer] });
  restore = fake.restore;
  await checkedWindow(client(), key, io().io);
  assert.deepEqual(fake.calls.order, ["check", "mark", "stored"]);
});

test("a stored window below a cursor is drawn before the check answers", async () => {
  const fake = setup({ stores: [storedWindow()] });
  restore = fake.restore;
  let answerCheck = () => {};
  const restoreCheck = setWindowCheckDeps({
    check: () =>
      new Promise((resolve) => {
        answerCheck = () =>
          resolve({ answer: equalAnswer, noCheck: false, page: null });
      }),
  });
  const painted = [];
  const page = io();
  page.io.paint = (events) => painted.push(events);
  const cursorKey = { ...key, cursor: { createdAt: 40, eventId: "d" } };
  const opening = checkedWindow(client(), cursorKey, page.io);
  await tick();
  assert.deepEqual(painted, [rows], "drawn while the check is out");
  answerCheck();
  assert.equal(await opening, rows);
  restoreCheck();
});

test("a newest window is not drawn by the check (its open paints)", async () => {
  const fake = setup({ stores: [storedWindow()], checks: [equalAnswer] });
  restore = fake.restore;
  const painted = [];
  const page = io();
  page.io.paint = (events) => painted.push(events);
  await checkedWindow(client(), key, page.io);
  assert.deepEqual(painted, []);
});

test("a thread's check compares again only on that thread's live rows", async () => {
  const threadKey = { ...key, rootId: "root" };
  const asked = [];
  const fake = setup({
    stores: [storedWindow({ count: 2 }), storedWindow()],
    checks: [equalAnswer],
    live: (windowKey) => {
      asked.push(windowKey);
      return 0;
    },
  });
  restore = fake.restore;
  await checkedWindow(client(), threadKey, io().io);
  assert.ok(asked.every((windowKey) => windowKey.rootId === "root"));
});

test("shadow fetches alongside the check, never after it", async () => {
  noteServerWindowCheckMode(RELAY, "shadow", 1_000);
  const fake = setup({ stores: [storedWindow()] });
  restore = fake.restore;
  let answerCheck = () => {};
  const restoreCheck = setWindowCheckDeps({
    check: () =>
      new Promise((resolve) => {
        answerCheck = () =>
          resolve({
            answer: { ...equalAnswer, mode: "shadow" },
            noCheck: false,
            page: null,
          });
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
    checks: [new Error("unknown filter field"), equalAnswer],
  });
  restore = fake.restore;
  const page = io();
  assert.equal(await checkedWindow(client(), threadKey, page.io), "relay");
  assert.deepEqual(page.fetches, [false]);
  assert.deepEqual(fake.calls.log, []);
  assert.deepEqual(fake.calls.release, ["mark"]);
  // An error says nothing about the relay: only this window took today's
  // path, the next open checks again.
  const next = io();
  await checkedWindow(client(), threadKey, next.io);
  assert.deepEqual(next.fetches, []);
  assert.equal(fake.calls.check, 2);
});

test("an empty answer (a room out of reach) leaves the relay's mode alone", async () => {
  resetWindowCheckModes();
  const fake = setup({
    stores: [storedWindow()],
    checks: [{ answer: null, noCheck: false, page: null }],
  });
  restore = fake.restore;
  await checkedWindow(client(), key, io().io);
  await tick();
  assert.equal(fake.calls.check, 1);
  // Still unknown: the next open asks again.
  await checkedWindow(client(), key, io().io);
  await tick();
  assert.equal(fake.calls.check, 2);
});

test("an old relay is asked once per ten minutes, behind today's request", async () => {
  resetWindowCheckModes();
  let now = 1_000;
  const oldPage = [row("c", 30), { id: "bounds", kind: 39006 }];
  const fake = setup({
    stores: [storedWindow()],
    checks: [{ answer: null, noCheck: true, page: oldPage }],
    now: () => now,
  });
  restore = fake.restore;
  const first = io();
  assert.equal(await checkedWindow(client(), key, first.io), "relay");
  assert.deepEqual(first.fetches, [false], "today's request, not held");
  await tick();
  assert.equal(fake.calls.check, 1);

  // Within ten minutes: today's request only.
  now += WINDOW_CHECK_REASK_MS - 1;
  const second = io();
  await checkedWindow(client(), key, second.io);
  assert.deepEqual(second.fetches, [false]);
  assert.equal(fake.calls.check, 1);

  // Then asked again, once.
  now += 1;
  await checkedWindow(client(), key, io().io);
  await tick();
  assert.equal(fake.calls.check, 2);
});

test("an old relay's page answer to a room check in mode on is the page", async () => {
  const oldPage = [row("c", 30), { id: "bounds", kind: 39006 }];
  const fake = setup({
    stores: [storedWindow()],
    checks: [{ answer: null, noCheck: true, page: oldPage }],
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
  // Now known off: today's request, no check.
  const next = io();
  await checkedWindow(client(), key, next.io);
  assert.deepEqual(next.fetches, [false]);
  assert.equal(fake.calls.check, 1);
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
  await tick();
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
      return { answer: equalAnswer, noCheck: false, page: null };
    },
  });
  const page = io();
  await checkedWindow(client(), key, page.io);
  restoreCheck();
  assert.equal(fake.calls.check, 2);
  assert.deepEqual(page.fetches, [], "equal on the second look");
});

test("a window the store does not hold is fetched and kept", async () => {
  const fake = setup({
    stores: [storedWindow({ fullyStored: false })],
    checks: [equalAnswer],
  });
  restore = fake.restore;
  const page = io();
  await checkedWindow(client(), key, page.io);
  // The check went out alongside the store read; its answer only notes
  // the relay's mode.
  assert.equal(fake.calls.check, 1);
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

test("a first open sends today's request at once, the check behind it", async () => {
  resetWindowCheckModes();
  const fake = setup({ stores: [storedWindow()] });
  restore = fake.restore;
  const restoreCheck = setWindowCheckDeps({
    check: () => new Promise(() => {}),
    stored: () => new Promise(() => {}),
  });
  const page = io();
  assert.equal(await checkedWindow(client(), key, page.io), "relay");
  assert.deepEqual(page.fetches, [false]);
  restoreCheck();
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
  // The check's mark and the fetch's.
  assert.deepEqual(fake.calls.release, ["mark", "mark"]);
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
  assert.deepEqual(fake.calls.release, ["mark", "mark"]);
});

test("a cache without an account and server is today's path", async () => {
  const fake = setup({ stores: [storedWindow()], checks: [equalAnswer] });
  restore = fake.restore;
  const page = io();
  await checkedWindow(new QueryClient(), key, page.io);
  assert.deepEqual(page.fetches, [false]);
  assert.equal(fake.calls.stored, 0);
});

test("a room page's rows are its timeline kinds, with has_more from its bounds", () => {
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
    ["c"],
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
