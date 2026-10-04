import assert from "node:assert/strict";
import test from "node:test";

import { autoAssignDecision, routableTextKey } from "./autoAssignGate.ts";

const OPEN = {
  routerActive: true,
  routerReady: true,
  channelType: "stream",
  isEditing: false,
  addressedAgentCount: 0,
  explicitMentionCount: 0,
  text: "fix the windows build",
  rosterSize: 3,
};

test("an unmentioned stream or forum draft with a roster routes", () => {
  assert.deepEqual(autoAssignDecision(OPEN), { run: true });
  assert.deepEqual(autoAssignDecision({ ...OPEN, channelType: "forum" }), {
    run: true,
  });
});

test("every skip reason, each alone on an otherwise routable draft", () => {
  const cases = [
    [{ routerActive: false }, "router-off"],
    [{ routerReady: false }, "router-not-ready"],
    [{ channelType: "dm" }, "channel-type"],
    [{ channelType: null }, "channel-type"],
    [{ channelType: undefined }, "channel-type"],
    [{ isEditing: true }, "editing"],
    [{ addressedAgentCount: 1 }, "addressed"],
    [{ explicitMentionCount: 1 }, "explicit-mention"],
    [{ text: "" }, "too-short"],
    [{ text: " k \n" }, "too-short"],
    [{ rosterSize: 0 }, "no-roster"],
  ];
  for (const [override, reason] of cases) {
    assert.deepEqual(
      autoAssignDecision({ ...OPEN, ...override }),
      { run: false, reason },
      JSON.stringify(override),
    );
  }
});

test("the applied-mode check comes first: any non-Smart-routing state never routes", () => {
  // Off, Host, Lead, and a switch in flight all report routerActive=false,
  // whatever else is true about the draft.
  const decision = autoAssignDecision({
    ...OPEN,
    routerActive: false,
    routerReady: false,
    channelType: "dm",
    isEditing: true,
  });
  assert.deepEqual(decision, { run: false, reason: "router-off" });
});

test("two non-blank characters are enough", () => {
  assert.deepEqual(autoAssignDecision({ ...OPEN, text: " o k " }), {
    run: true,
  });
});

test("routableTextKey trims and collapses whitespace only", () => {
  assert.equal(routableTextKey("  fix   the\nbuild  "), "fix the build");
  assert.equal(routableTextKey("Fix"), "Fix");
  assert.equal(routableTextKey(" \n "), "");
});
