import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  chunkRoomIds,
  ROOM_LIVE_KINDS,
  RoomLiveBundle,
} from "./relayRoomBundle.ts";
import {
  CHANNEL_EVENT_KINDS,
  KIND_CHANNEL_THREAD_SUMMARY,
} from "@/shared/constants/kinds";

const flush = async () => {
  for (let i = 0; i < 20; i++) await Promise.resolve();
};

/** A bundle over a fake relay: each opened chunk is held until released. */
function harness({ autoOpen = true } = {}) {
  const opened = [];
  const timers = new Map();
  let nextTimer = 1;
  const bundle = new RoomLiveBundle(
    (filter, onEvent) => {
      const sub = { filter, onEvent, disposed: false };
      opened.push(sub);
      return new Promise((resolve, reject) => {
        sub.open = () =>
          resolve(async () => {
            sub.disposed = true;
          });
        sub.fail = (error = new Error("fixture setup failure")) =>
          reject(error);
        if (autoOpen) sub.open();
      });
    },
    {
      setTimeout: (fn, ms) => {
        timers.set(nextTimer, { fn, ms });
        return nextTimer++;
      },
      clearTimeout: (id) => timers.delete(id),
    },
  );
  const live = () => opened.filter((sub) => !sub.disposed);
  return { bundle, opened, live, timers };
}

function event(id, roomId, overrides = {}) {
  return {
    id,
    kind: 9,
    pubkey: "b".repeat(64),
    content: id,
    created_at: 100,
    tags: roomId ? [["h", roomId]] : [],
    sig: "",
    ...overrides,
  };
}

const rooms = (count, prefix = "room") =>
  Array.from(
    { length: count },
    (_, index) => `${prefix}-${String(index).padStart(3, "0")}`,
  );

test("member rooms share one live REQ per 128 rooms; extras stay apart", async () => {
  const { bundle, opened } = harness();
  bundle.setMemberRooms(rooms(130));
  const stop = await bundle.listenRoom("browsed", () => {});
  await flush();
  assert.deepEqual(
    opened.map((sub) => sub.filter["#h"].length),
    [128, 2, 1],
  );
  assert.deepEqual(opened[2].filter["#h"], ["browsed"]);
  for (const sub of opened) {
    assert.equal(sub.filter.limit, 0, "live only, no history");
    assert.equal(
      sub.filter.since,
      undefined,
      "since would drop backdated events",
    );
    assert.deepEqual(sub.filter.kinds, [...ROOM_LIVE_KINDS]);
  }
  assert.ok(
    CHANNEL_EVENT_KINDS.every((kind) => ROOM_LIVE_KINDS.includes(kind)),
  );
  assert.ok(ROOM_LIVE_KINDS.includes(KIND_CHANNEL_THREAD_SUMMARY));
  assert.deepEqual(chunkRoomIds(["b", "a", "a"]), [["a", "b"]]);
  await stop();
});

test("a listened member room needs no REQ of its own", async () => {
  const { bundle, opened } = harness();
  bundle.setMemberRooms(["general", "random"]);
  await flush();
  const stop = await bundle.listenRoom("general", () => {});
  await flush();
  assert.equal(opened.length, 1);
  await stop();
  await flush();
  assert.equal(opened.length, 1);
  assert.equal(opened[0].disposed, false);
});

test("room listeners get their room's rows, replies and summaries; all-room listeners never get summaries", async () => {
  const { bundle, opened } = harness();
  bundle.setMemberRooms(["general", "random"]);
  const roomEvents = [];
  const allEvents = [];
  bundle.listenAllRooms((e) => allEvents.push(e.id));
  const stop = await bundle.listenRoom("general", (e) => roomEvents.push(e.id));
  await flush();
  const [sub] = opened;
  const reply = event("reply", "general", {
    tags: [
      ["h", "general"],
      ["e", "root", "", "root"],
      ["e", "root", "", "reply"],
    ],
  });
  sub.onEvent(event("row", "general"));
  sub.onEvent(reply);
  sub.onEvent(
    event("summary", "general", {
      kind: KIND_CHANNEL_THREAD_SUMMARY,
      tags: [
        ["e", "root"],
        ["d", "root"],
        ["h", "general"],
      ],
    }),
  );
  sub.onEvent(event("elsewhere", "random"));
  assert.deepEqual(roomEvents, ["row", "reply", "summary"]);
  assert.deepEqual(allEvents, ["row", "reply", "elsewhere"]);
  await stop();
});

test("a browsed room's events reach its listener but not the all-room listeners", async () => {
  const { bundle, opened } = harness();
  bundle.setMemberRooms(["general"]);
  const allEvents = [];
  const roomEvents = [];
  bundle.listenAllRooms((e) => allEvents.push(e.id));
  const stop = await bundle.listenRoom("browsed", (e) => roomEvents.push(e.id));
  await flush();
  const browsed = opened.find((sub) => sub.filter["#h"][0] === "browsed");
  browsed.onEvent(event("visit", "browsed"));
  assert.deepEqual(roomEvents, ["visit"]);
  assert.deepEqual(allEvents, []);
  await stop();
  await flush();
  assert.equal(browsed.disposed, true, "leaving the room closes its REQ");
});

test("listenRoom resolves only once a live chunk covers the room", async () => {
  const { bundle, opened } = harness({ autoOpen: false });
  bundle.setMemberRooms(["general"]);
  let resolved = false;
  const pending = bundle
    .listenRoom("general", () => {})
    .then((stop) => {
      resolved = true;
      return stop;
    });
  await flush();
  assert.equal(resolved, false);
  opened[0].open();
  const stop = await pending;
  assert.equal(resolved, true);
  await stop();
});

test("a replaced chunk stays open until its successor is live, and overlap delivers once", async () => {
  const { bundle, opened } = harness({ autoOpen: false });
  const delivered = [];
  bundle.listenAllRooms((e) => delivered.push(e.id));
  bundle.setMemberRooms(["a", "b"]);
  await flush();
  opened[0].open();
  await flush();

  bundle.setMemberRooms(["a", "b", "c"]);
  await flush();
  assert.equal(opened.length, 2);
  assert.equal(opened[0].disposed, false, "old chunk covers a and b meanwhile");

  const shared = event("shared", "a");
  opened[0].onEvent(shared);
  opened[1].onEvent(shared);
  assert.deepEqual(delivered, ["shared"]);

  opened[1].open();
  await flush();
  assert.equal(opened[0].disposed, true, "retired once the new chunk is live");
  opened[0].onEvent(event("late", "a"));
  opened[1].onEvent(event("fresh", "c"));
  assert.deepEqual(delivered, ["shared", "fresh"]);
});

test("removing every room retires a pending chunk without delivering its events", async () => {
  const { bundle, opened } = harness({ autoOpen: false });
  const delivered = [];
  bundle.listenAllRooms((e) => delivered.push(e.id));
  bundle.setMemberRooms(["a"]);
  await flush();
  bundle.setMemberRooms([]);
  await flush();
  opened[0].open();
  await flush();
  assert.equal(opened[0].disposed, true);
  opened[0].onEvent(event("late", "a"));
  assert.deepEqual(delivered, []);
});

test("h-less reactions find their room: single-room chunk, earlier delivery, or the loaded timelines", async () => {
  const { bundle, opened } = harness();
  bundle.setMemberRooms(["a", "b"]);
  const roomA = [];
  const roomB = [];
  const all = [];
  bundle.listenAllRooms((e) =>
    all.push([e.id, e.tags.find((t) => t[0] === "h")?.[1]]),
  );
  const stopA = await bundle.listenRoom("a", (e) => roomA.push(e.id));
  const stopB = await bundle.listenRoom("b", (e) => roomB.push(e.id));
  const stopBrowsed = await bundle.listenRoom("browsed", () => {});
  await flush();
  const shared = opened.find((sub) => sub.filter["#h"].length === 2);
  const single = opened.find((sub) => sub.filter["#h"][0] === "browsed");

  const reaction = (id, target) =>
    event(id, null, { kind: 7, content: "👍", tags: [["e", target]] });

  // Seen earlier on this bundle.
  shared.onEvent(event("row-a", "a"));
  shared.onEvent(reaction("like-row-a", "row-a"));
  // Removing that reaction references the reaction, two hops from the row.
  shared.onEvent(
    event("unlike", null, { kind: 5, tags: [["e", "like-row-a"]] }),
  );

  // Known only to the loaded timelines.
  bundle.setRoomResolver((id) => (id === "paged-b" ? "b" : null));
  shared.onEvent(reaction("like-paged-b", "paged-b"));

  // Unknown target: dropped, but not remembered as seen.
  shared.onEvent(reaction("like-unknown", "unknown"));
  bundle.setRoomResolver((id) => (id === "unknown" ? "a" : null));
  shared.onEvent(reaction("like-unknown", "unknown"));

  // A single-room chunk names its room.
  const browsedEvents = [];
  const stopBrowsed2 = await bundle.listenRoom("browsed", (e) =>
    browsedEvents.push(e.id),
  );
  single.onEvent(reaction("like-browsed", "never-seen"));

  assert.deepEqual(roomA, ["row-a", "like-row-a", "unlike", "like-unknown"]);
  assert.deepEqual(roomB, ["like-paged-b"]);
  assert.deepEqual(browsedEvents, ["like-browsed"]);
  assert.deepEqual(all, [
    ["row-a", "a"],
    ["like-row-a", "a"],
    ["unlike", "a"],
    ["like-paged-b", "b"],
    ["like-unknown", "a"],
  ]);
  for (const stop of [stopA, stopB, stopBrowsed, stopBrowsed2]) await stop();
});

test("a resolver naming a room outside the chunk does not misroute the event", async () => {
  const { bundle, opened } = harness();
  bundle.setMemberRooms(["a", "b"]);
  const all = [];
  bundle.listenAllRooms((e) => all.push(e.id));
  bundle.setRoomResolver(() => "elsewhere");
  await flush();
  opened[0].onEvent(event("like", null, { kind: 7, tags: [["e", "x"]] }));
  assert.deepEqual(all, []);
});

test("a failed open settles its listeners and retries with backoff while wanted", async () => {
  const { bundle, opened, timers } = harness({ autoOpen: false });
  bundle.setMemberRooms(["a"]);
  const pending = bundle.listenRoom("a", () => {});
  await flush();
  opened[0].fail();
  const stop = await pending;
  assert.equal(timers.size, 1);
  assert.equal([...timers.values()][0].ms, 1_000);
  [...timers.values()][0].fn();
  timers.clear();
  await flush();
  assert.equal(opened.length, 2, "same rooms requested again");
  assert.deepEqual(opened[1].filter["#h"], ["a"]);
  opened[1].fail();
  await flush();
  assert.equal([...timers.values()][0].ms, 2_000, "backoff grows");

  bundle.setMemberRooms([]);
  await stop();
  await flush();
  const retry = [...timers.values()][0];
  retry?.fn();
  await flush();
  assert.equal(opened.length, 2, "a room no longer wanted is not retried");
});

test("reset forgets delivered events and reopens rooms still listened to", async () => {
  const { bundle, opened } = harness();
  bundle.setMemberRooms(["a"]);
  const roomEvents = [];
  const stop = await bundle.listenRoom("a", (e) => roomEvents.push(e.id));
  await flush();
  opened[0].onEvent(event("once", "a"));
  bundle.reset();
  await flush();
  assert.equal(opened[0].disposed, true);
  const reopened = opened.at(-1);
  assert.deepEqual(reopened.filter["#h"], ["a"]);
  reopened.onEvent(event("once", "a"));
  assert.deepEqual(roomEvents, ["once", "once"]);
  await stop();
});

// ── Shared rules with mobile (test-fixtures/room-live-routing.json) ──────────

const fixtures = JSON.parse(
  readFileSync(
    new URL(
      "../../../../test-fixtures/room-live-routing.json",
      import.meta.url,
    ),
    "utf8",
  ),
);

test("shared fixture: chunking matches mobile", () => {
  for (const fixture of fixtures.chunking) {
    const rooms = fixture.generate
      ? Array.from(
          { length: fixture.generate.count },
          (_, index) => `${fixture.generate.prefix}${index}`,
        )
      : fixture.rooms;
    const chunks = chunkRoomIds(rooms);
    if (fixture.chunks) assert.deepEqual(chunks, fixture.chunks, fixture.name);
    if (fixture.chunkSizes) {
      assert.deepEqual(
        chunks.map((chunk) => chunk.length),
        fixture.chunkSizes,
        fixture.name,
      );
    }
    assert.ok(
      chunks.every((chunk) => chunk.length <= fixtures.maxRoomsPerSubscription),
    );
  }
});

for (const fixture of fixtures.routing) {
  test(`shared fixture: ${fixture.name}`, async () => {
    const { bundle, opened } = harness();
    const loaded = fixture.loaded ?? {};
    bundle.setMemberRooms(fixture.chunkRooms);
    bundle.setRoomResolver(
      (eventId) =>
        Object.entries(loaded).find(([, ids]) => ids.includes(eventId))?.[0] ??
        null,
    );
    const placed = [];
    const rooms = new Set([...fixture.chunkRooms, ...Object.keys(loaded)]);
    const stops = [];
    for (const roomId of rooms) {
      stops.push(
        await bundle.listenRoom(roomId, (e) => {
          if (e.id === fixture.event.id) placed.push(roomId);
        }),
      );
    }
    await flush();
    const chunk = opened.find(
      (sub) =>
        sub.filter["#h"].join(",") === [...fixture.chunkRooms].sort().join(","),
    );
    for (const before of fixture.routedBefore ?? []) {
      chunk.onEvent(event(before.id, null, before));
    }
    chunk.onEvent(event(fixture.event.id, null, fixture.event));
    assert.deepEqual(
      placed,
      fixture.expectedRoom === null ? [] : [fixture.expectedRoom],
    );
    for (const stop of stops) await stop();
  });
}
