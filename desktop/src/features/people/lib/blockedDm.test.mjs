import assert from "node:assert/strict";
import test from "node:test";

import {
  blockedDmPeer,
  foldsBlockedAuthors,
  isOneToOneDm,
} from "./blockedDm.ts";

const ME = "a".repeat(64);
const BOB = "b".repeat(64);
const CAROL = "c".repeat(64);

const dm = (participantPubkeys) => ({ channelType: "dm", participantPubkeys });
const isBob = (pubkey) => pubkey === BOB;

test("a DM with two distinct people (or one) is 1:1", () => {
  assert.equal(isOneToOneDm(dm([ME, BOB])), true);
  assert.equal(isOneToOneDm(dm([ME])), true);
  assert.equal(isOneToOneDm(dm([ME, BOB, BOB.toUpperCase()])), true);
  assert.equal(isOneToOneDm(dm([ME, BOB, CAROL])), false);
  assert.equal(
    isOneToOneDm({ channelType: "stream", participantPubkeys: [ME, BOB] }),
    false,
  );
  assert.equal(isOneToOneDm(null), false);
});

test("only a 1:1 DM leaves blocked authors unfolded", () => {
  assert.equal(foldsBlockedAuthors(dm([ME, BOB])), false);
  assert.equal(foldsBlockedAuthors(dm([ME, BOB, CAROL])), true);
  assert.equal(
    foldsBlockedAuthors({ channelType: "stream", participantPubkeys: [] }),
    true,
  );
  assert.equal(foldsBlockedAuthors(null), true);
});

test("the blocked peer is the other person of a 1:1 DM I blocked", () => {
  assert.equal(blockedDmPeer(dm([ME, BOB]), ME, isBob), BOB);
  assert.equal(
    blockedDmPeer(dm([BOB.toUpperCase(), ME]), ME.toUpperCase(), isBob),
    BOB,
  );
  assert.equal(blockedDmPeer(dm([ME, CAROL]), ME, isBob), null);
  assert.equal(blockedDmPeer(dm([ME, BOB, CAROL]), ME, isBob), null);
  assert.equal(
    blockedDmPeer(dm([ME]), ME, () => true),
    null,
  );
  assert.equal(blockedDmPeer(dm([ME, BOB]), undefined, isBob), null);
  assert.equal(blockedDmPeer(null, ME, isBob), null);
});
