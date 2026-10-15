import assert from "node:assert/strict";
import test from "node:test";

import {
  buildDirectMessageIntro,
  formatDmParticipantLabels,
} from "./dmParticipantDisplay.ts";

const ME = "a".repeat(64);
const ALICE = "b".repeat(64);
const U1 = "c".repeat(64);
const U2 = "d".repeat(64);

function dm(pubkeys) {
  return {
    channelType: "dm",
    name: "DM",
    participantPubkeys: pubkeys,
    participants: pubkeys.map(() => null),
  };
}

test("unnamed people share one counted entry", () => {
  assert.equal(
    formatDmParticipantLabels(["Alice", null, null]),
    "Alice, Unknown user (2)",
  );
  assert.equal(formatDmParticipantLabels([null]), "Unknown user");
});

test("+N more counts hidden people, not entries", () => {
  assert.equal(
    formatDmParticipantLabels(["A", "B", "C", "D", null, null]),
    "A, B, C, +3 more",
  );
});

test("DM intro title counts unnamed participants and shows no keys", () => {
  const intro = buildDirectMessageIntro({
    channel: dm([ME, ALICE, U1, U2]),
    currentPubkey: ME,
    profiles: { [ALICE]: { displayName: "Alice" } },
  });
  assert.equal(intro.displayName, "Alice, Unknown user (2)");
  assert.equal(intro.participants.length, 3);
  for (const participant of intro.participants) {
    assert.doesNotMatch(participant.displayName, /[0-9a-f]{8}|npub/);
  }
});
