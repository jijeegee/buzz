// A live join / accept / leave in the open chat must refresh its member list
// (title, grid avatar, Chat info) even though the relay publishes the system
// message before the new member list.
import assert from "node:assert/strict";
import test from "node:test";

import {
  MEMBERSHIP_SETTLE_DELAYS_MS,
  refreshOnSystemMessage,
} from "./membershipSystemRefresh.ts";

function harness() {
  const invalidations = [];
  const notes = [];
  const timers = [];
  const deps = {
    queryClient: {
      invalidateQueries: async (filters) => {
        invalidations.push(filters);
      },
    },
    noteMembershipChange: (channelId) => notes.push(channelId),
    setTimer: (callback, delayMs) => timers.push({ callback, delayMs }),
  };
  return { deps, invalidations, notes, timers };
}

for (const type of ["member_joined", "member_left", "member_removed"]) {
  test(`${type} refreshes now and again after the member list settles`, () => {
    const { deps, invalidations, notes, timers } = harness();
    const handled = refreshOnSystemMessage(
      "room-1",
      JSON.stringify({ type, actor: "aa", target: "bb" }),
      deps,
    );
    assert.equal(handled, true);
    assert.deepEqual(notes, ["room-1"]);
    assert.deepEqual(invalidations, [{ queryKey: ["channels"], exact: true }]);
    assert.deepEqual(
      timers.map((timer) => timer.delayMs),
      [...MEMBERSHIP_SETTLE_DELAYS_MS],
    );

    for (const timer of timers) timer.callback();
    assert.deepEqual(notes, ["room-1", "room-1", "room-1"]);
    assert.equal(invalidations.length, 3);
  });
}

test("picture change refreshes the list once without roster refresh", () => {
  const { deps, invalidations, notes, timers } = harness();
  const handled = refreshOnSystemMessage(
    "room-1",
    JSON.stringify({ type: "picture_changed" }),
    deps,
  );
  assert.equal(handled, false);
  assert.deepEqual(invalidations, [{ queryKey: ["channels"], exact: true }]);
  assert.deepEqual(notes, []);
  assert.deepEqual(timers, []);
});

test("other and malformed system messages refresh nothing", () => {
  const { deps, invalidations, notes, timers } = harness();
  assert.equal(
    refreshOnSystemMessage("room-1", '{"type":"topic_changed"}', deps),
    false,
  );
  assert.equal(refreshOnSystemMessage("room-1", "not json", deps), false);
  assert.deepEqual(invalidations, []);
  assert.deepEqual(notes, []);
  assert.deepEqual(timers, []);
});
