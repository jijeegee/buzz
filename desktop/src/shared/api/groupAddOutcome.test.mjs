import assert from "node:assert/strict";
import test from "node:test";

import {
  classifyMemberAddError,
  INVITATIONS_RATE_LIMITED_TEXT,
  invitationSentText,
  splitMemberAddResult,
} from "./groupAddOutcome.ts";

test("classifies relay add-member replies", () => {
  assert.equal(
    classifyMemberAddError(
      "relay rejected event: invite-sent: invitation stored",
    ),
    "invite-sent",
  );
  assert.equal(
    classifyMemberAddError("invite-sent: invitation stored"),
    "invite-sent",
  );
  assert.equal(
    classifyMemberAddError(
      "relay rejected event: rate-limited: too many invitations, try later",
    ),
    "rate-limited",
  );
  assert.equal(
    classifyMemberAddError("relay rejected event: restricted: not allowed"),
    "error",
  );
});

test("splits invited people from real errors", () => {
  const result = splitMemberAddResult({
    added: ["a"],
    errors: [
      { pubkey: "b", error: "relay rejected event: invite-sent: stored" },
      {
        pubkey: "c",
        error:
          "relay rejected event: rate-limited: too many invitations, try later",
      },
      { pubkey: "d", error: "relay rejected event: restricted: nope" },
    ],
  });
  assert.deepEqual(result.added, ["a"]);
  assert.deepEqual(result.invited, ["b"]);
  assert.deepEqual(result.errors, [
    { pubkey: "c", error: INVITATIONS_RATE_LIMITED_TEXT },
    { pubkey: "d", error: "relay rejected event: restricted: nope" },
  ]);
});

test("user-facing texts", () => {
  assert.equal(invitationSentText("Alice"), "Invitation sent to Alice");
  assert.equal(
    invitationSentText("Alice, Bob"),
    "Invitation sent to Alice, Bob",
  );
  assert.equal(
    INVITATIONS_RATE_LIMITED_TEXT,
    "Too many invitations. Try again later.",
  );
});
