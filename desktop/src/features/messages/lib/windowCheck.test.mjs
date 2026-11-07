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

const RELAY = "wss://relay.test";
const key = { roomId: "room", rootId: null, cursor: null, limit: 3 };

const row = (id, createdAt) => ({ id, kind: 9, created_at: createdAt });
const rows = [row("c", 30), row("b", 20), row("a", 10)];

function storedWindow(overrides = {}) {
  return {
    events: ["stored"],
    count: 3,
    oldest: { createdAt: 10, eventId: "a" },
    hasMore: true,
    fullyStored: true,
    ...overrides,
  };
}

/** A fake relay and store; `stores` are read one after another. */
function setup({ stores, checks = [], live = () => 0, now = () => 1_000 }) {
  const calls = { stored: 0, check: 0, replace: [], release: [], log: [] };
  const restore = setWindowCheckDeps({
    stored: async () => {
      const next = stores[Math.min(calls.stored, stores.length - 1)];
      calls.stored += 1;
      return next;
    },
    check: async () => {
      const next = checks[Math.min(calls.check, checks.length - 1)];
      calls.check += 1;
      return next ?? null;
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
  assert.deepEqual(answer, ["stored"]);
  assert.deepEqual(page.fetches, []);
  assert.equal(fake.calls.check, 1);
  assert.deepEqual(fake.calls.replace, []);
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

test("shadow fetches an equal window anyway and logs whether it agreed", async () => {
  const fake = setup({
    stores: [storedWindow()],
    checks: [{ ...equalAnswer, mode: "shadow" }],
  });
  restore = fake.restore;
  const page = io();
  assert.equal(await checkedWindow(client(), key, page.io), "relay");
  assert.deepEqual(page.fetches, [false]);
  assert.deepEqual(fake.calls.replace, []);
  assert.match(
    fake.calls.log.join("\n"),
    /shadow room room equal, page agrees/,
  );
});

test("shadow refetches and replaces a different window like on", async () => {
  const fake = setup({
    stores: [storedWindow({ count: 2 }), storedWindow()],
    checks: [{ ...equalAnswer, mode: "shadow" }],
  });
  restore = fake.restore;
  const page = io();
  await checkedWindow(client(), key, page.io);
  assert.deepEqual(page.fetches, [true]);
  assert.equal(fake.calls.replace.length, 1);
  assert.match(fake.calls.log.join("\n"), /differ, refetched/);
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
      return equalAnswer;
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
