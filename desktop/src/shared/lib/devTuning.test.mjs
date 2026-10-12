import assert from "node:assert/strict";
import test from "node:test";

import { DEFAULT_DEV_TUNING, parseDevTuning } from "./devTuning.ts";

test("a missing or malformed file means the defaults", () => {
  assert.deepEqual(parseDevTuning(null), DEFAULT_DEV_TUNING);
  assert.deepEqual(parseDevTuning("not json"), DEFAULT_DEV_TUNING);
  assert.deepEqual(parseDevTuning("[]"), DEFAULT_DEV_TUNING);
  assert.deepEqual(parseDevTuning('{"warmRooms":"5"}'), DEFAULT_DEV_TUNING);
});

test("warmRooms is read, floored and kept within 0..10", () => {
  assert.equal(parseDevTuning('{"warmRooms":5}').warmRooms, 5);
  assert.equal(parseDevTuning('{"warmRooms":2.7}').warmRooms, 2);
  assert.equal(parseDevTuning('{"warmRooms":-1}').warmRooms, 0);
  assert.equal(parseDevTuning('{"warmRooms":99}').warmRooms, 10);
});

test("preload wave sizes are read with their own defaults", () => {
  const tuning = parseDevTuning('{"preloadFirstRows":8,"preloadMoreRows":80}');
  assert.equal(tuning.preloadFirstRows, 8);
  assert.equal(tuning.preloadMoreRows, 50);
  assert.equal(tuning.warmRooms, DEFAULT_DEV_TUNING.warmRooms);
});
