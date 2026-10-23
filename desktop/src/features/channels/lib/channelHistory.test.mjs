import assert from "node:assert/strict";
import test from "node:test";

import {
  canToggleChannelHistory,
  historyForSwitch,
  newMembersSeeHistory,
} from "./channelHistory.ts";

const group = { channelType: "stream", groupChat: false };

test("a group shows earlier messages only when history is all", () => {
  assert.equal(newMembersSeeHistory({ ...group, history: "all" }), true);
  assert.equal(
    newMembersSeeHistory({ ...group, history: "since_join" }),
    false,
  );
  assert.equal(newMembersSeeHistory({ ...group, history: null }), false);
  assert.equal(newMembersSeeHistory({ ...group }), false);
  assert.equal(newMembersSeeHistory(null), false);
});

test("DMs and group chats never show earlier messages", () => {
  assert.equal(
    newMembersSeeHistory({ channelType: "dm", history: "all" }),
    false,
  );
  assert.equal(
    newMembersSeeHistory({ ...group, groupChat: true, history: "all" }),
    false,
  );
});

test("the switch saves all when on and since_join when off", () => {
  assert.equal(historyForSwitch(true), "all");
  assert.equal(historyForSwitch(false), "since_join");
});

test("only owners and admins of a group get the switch", () => {
  assert.equal(canToggleChannelHistory(group, true), true);
  assert.equal(canToggleChannelHistory(group, false), false);
  assert.equal(canToggleChannelHistory({ channelType: "dm" }, true), false);
  assert.equal(
    canToggleChannelHistory({ ...group, groupChat: true }, true),
    false,
  );
  assert.equal(canToggleChannelHistory(null, true), false);
});
