import assert from "node:assert/strict";
import test from "node:test";

import { nextWarmRooms } from "./warmRooms.ts";

const start = { recency: ["a"], order: ["a"] };

test("rooms join in visit order and keep their render order", () => {
  let rooms = nextWarmRooms(start, "b", 3);
  rooms = nextWarmRooms(rooms, "c", 3);
  assert.deepEqual(rooms.order, ["a", "b", "c"]);
  // Revisiting reorders recency only; rendered rooms never move.
  rooms = nextWarmRooms(rooms, "a", 3);
  rooms = nextWarmRooms(rooms, "b", 3);
  assert.deepEqual(rooms.order, ["a", "b", "c"]);
  assert.deepEqual(rooms.recency, ["b", "a", "c"]);
});

test("the least recently open room is dropped for a new one", () => {
  let rooms = nextWarmRooms(start, "b", 3);
  rooms = nextWarmRooms(rooms, "c", 3);
  rooms = nextWarmRooms(rooms, "a", 3);
  rooms = nextWarmRooms(rooms, "d", 3);
  assert.deepEqual(rooms.order, ["a", "c", "d"]);
  assert.deepEqual(rooms.recency, ["d", "a", "c"]);
});

test("one warm room keeps only the open room", () => {
  const rooms = nextWarmRooms(nextWarmRooms(start, "b", 1), "c", 1);
  assert.deepEqual(rooms.order, ["c"]);
});
