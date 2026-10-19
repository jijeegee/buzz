import assert from "node:assert/strict";
import test from "node:test";

import {
  INVITATION_NO_LONGER_VALID_TEXT,
  invitationsRowLabel,
  invitationsViewState,
  invitedYouText,
  memberCountLabel,
  withoutInvitation,
} from "./invitationsState.ts";

function invitation(id) {
  return {
    id,
    channel: {
      id: `g-${id}`,
      name: `Group ${id}`,
      description: null,
      channelType: "stream",
      memberCount: 3,
      visibility: "private",
    },
    inviter: "ab".repeat(32),
    createdAt: "2026-10-20T00:00:00Z",
  };
}

test("view states: loading, error, empty, list", () => {
  assert.deepEqual(
    invitationsViewState({ data: undefined, isError: false, isLoading: true }),
    { kind: "loading" },
  );
  assert.deepEqual(
    invitationsViewState({ data: undefined, isError: true, isLoading: false }),
    { kind: "error" },
  );
  assert.deepEqual(
    invitationsViewState({ data: [], isError: false, isLoading: false }),
    { kind: "empty" },
  );
  const list = [invitation("1")];
  assert.deepEqual(
    invitationsViewState({ data: list, isError: true, isLoading: false }),
    { kind: "list", invitations: list },
  );
});

test("Chats row label hides at zero", () => {
  assert.equal(invitationsRowLabel(undefined), null);
  assert.equal(invitationsRowLabel([]), null);
  assert.equal(
    invitationsRowLabel([invitation("1"), invitation("2")]),
    "Invitations · 2",
  );
});

test("row texts", () => {
  assert.equal(memberCountLabel(1), "1 member");
  assert.equal(memberCountLabel(5), "5 members");
  assert.equal(invitedYouText("Alice"), "Alice invited you");
  assert.equal(
    INVITATION_NO_LONGER_VALID_TEXT,
    "This invitation is no longer valid.",
  );
});

test("removing an answered or invalid invitation", () => {
  const list = [invitation("1"), invitation("2")];
  assert.deepEqual(
    withoutInvitation(list, "1").map((item) => item.id),
    ["2"],
  );
  assert.deepEqual(withoutInvitation(undefined, "1"), []);
});
