import assert from "node:assert/strict";
import test from "node:test";

import {
  answerEachFilter,
  mockLiveFiltersMatch,
} from "./e2eBridgeMultiFilter.ts";

const ME = "a".repeat(64);
const OTHER = "b".repeat(64);
const event = (id, kind, pubkey, tags = []) => ({
  id,
  kind,
  pubkey,
  tags,
  content: "",
  created_at: 1,
  sig: "",
});

test("a live REQ hears an event any of its filters matches, and only then", () => {
  const filters = [
    { kinds: [30315], "#d": ["general"], limit: 0 },
    { kinds: [20001], authors: [ME], limit: 0 },
  ];
  assert.equal(
    mockLiveFiltersMatch(filters, event("1", 30315, OTHER, [["d", "general"]])),
    true,
  );
  assert.equal(
    mockLiveFiltersMatch(filters, event("2", 30315, OTHER, [["d", "other"]])),
    false,
  );
  assert.equal(mockLiveFiltersMatch(filters, event("3", 20001, ME)), true);
  // The relay filters presence by author; so does the mock now.
  assert.equal(mockLiveFiltersMatch(filters, event("4", 20001, OTHER)), false);
  // Room filters keep the kinds-only rule.
  assert.equal(
    mockLiveFiltersMatch([{ kinds: [9], "#h": ["room"], limit: 0 }], {
      ...event("5", 9, OTHER),
    }),
    true,
  );
});

test("a multi-filter one-shot answers every filter once, then one EOSE", () => {
  const frames = [];
  const shared = event("s", 1, ME);
  answerEachFilter(
    ["a", "b"],
    (frame) => frames.push(frame),
    (filter, send) => {
      send(["EVENT", "h-1", shared]);
      send(["EVENT", "h-1", event(filter, 1, ME)]);
      send(["EOSE", "h-1"]);
    },
  );
  assert.deepEqual(
    frames.map((frame) => (frame[0] === "EVENT" ? frame[2].id : frame[0])),
    ["s", "a", "b", "EOSE"],
  );
});

test("a CLOSED on any filter ends the multi-filter one-shot", () => {
  const frames = [];
  answerEachFilter(
    ["a", "b"],
    (frame) => frames.push(frame),
    (filter, send) => {
      if (filter === "a") send(["CLOSED", "h-1", "nope"]);
      else send(["EOSE", "h-1"]);
    },
  );
  assert.deepEqual(frames, [["CLOSED", "h-1", "nope"]]);
});
