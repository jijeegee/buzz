import assert from "node:assert/strict";
import test from "node:test";

import {
  accountAfterSignOut,
  accountEmailDetail,
  accountLabel,
  agentsOfAccount,
  canSwitchTo,
  menuAccounts,
} from "./accountLabels.ts";

function account(pubkey, overrides = {}) {
  return {
    pubkey,
    origin: "https://relay.example",
    state: "signed_in",
    active: false,
    displayName: null,
    buzzName: null,
    avatarUrl: null,
    email: null,
    lastUsedAt: null,
    ...overrides,
  };
}

test("an account goes by its name, else its email, else a short id", () => {
  const pubkey = "ab12cd34".repeat(8);
  assert.equal(
    accountLabel(account(pubkey, { displayName: " Jiho ", email: "j@x.io" })),
    "Jiho",
  );
  assert.equal(
    accountLabel(account(pubkey, { displayName: "  ", email: "j@x.io" })),
    "j@x.io",
  );
  assert.equal(accountLabel(account(pubkey)), "Account ab12cd34");
});

test("label priority: Buzz name, Google name, email, short id", () => {
  const pubkey = "ab12cd34".repeat(8);
  const rows = [
    [
      { buzzName: "akak", displayName: "akauuzk ka", email: "a@x.io" },
      "akak",
      "a@x.io",
    ],
    [
      { buzzName: " ", displayName: "akauuzk ka", email: "a@x.io" },
      "akauuzk ka",
      "a@x.io",
    ],
    [{ email: "a@x.io" }, "a@x.io", null],
    [{}, "Account ab12cd34", null],
  ];
  for (const [fields, label, detail] of rows) {
    const row = account(pubkey, fields);
    assert.equal(accountLabel(row), label);
    assert.equal(accountEmailDetail(row), detail, "email only when it differs");
  }
});

test("sign-out switches to the first other signed-in account on the same server", () => {
  const list = [
    account("a".repeat(64), { active: true }),
    account("b".repeat(64), { state: "needs_login" }),
    account("c".repeat(64), { origin: "https://other.example" }),
    account("d".repeat(64)),
    account("e".repeat(64)),
  ];
  assert.equal(accountAfterSignOut(list)?.pubkey[0], "d");
  assert.equal(accountAfterSignOut(list.slice(0, 3)), null);
  assert.equal(accountAfterSignOut(list.slice(1)), null, "no active account");
});

test("the menu keeps the backend's most-recent order and leaves out signed-out accounts", () => {
  const list = [
    account("a".repeat(64), { active: true }),
    account("b".repeat(64), { state: "signed_out" }),
    account("c".repeat(64), { state: "needs_login" }),
    account("d".repeat(64), { state: "key_only" }),
  ];
  assert.deepEqual(
    menuAccounts(list).map((a) => a.pubkey[0]),
    ["a", "c", "d"],
  );
});

test("only another signed-in (or key-only) account is a direct switch", () => {
  assert.equal(canSwitchTo(account("a".repeat(64))), true);
  assert.equal(
    canSwitchTo(account("a".repeat(64), { state: "key_only" })),
    true,
  );
  assert.equal(
    canSwitchTo(account("a".repeat(64), { active: true })),
    false,
    "already active",
  );
  assert.equal(
    canSwitchTo(account("a".repeat(64), { state: "needs_login" })),
    false,
    "needs a fresh sign-in first",
  );
  assert.equal(
    canSwitchTo(account("a".repeat(64), { state: "signed_out" })),
    false,
  );
});

test("each account sees its own agents and every ownerless one", () => {
  const me = "a".repeat(64);
  const agents = [
    { pubkey: "1", ownerPubkey: me },
    { pubkey: "2", ownerPubkey: "b".repeat(64) },
    { pubkey: "3", ownerPubkey: null },
    { pubkey: "4", ownerPubkey: me.toUpperCase() },
  ];
  assert.deepEqual(
    agentsOfAccount(agents, me).map((a) => a.pubkey),
    ["1", "3", "4"],
  );
  assert.deepEqual(
    agentsOfAccount(agents, undefined).map((a) => a.pubkey),
    ["1", "2", "3", "4"],
    "everything while the active account is not known",
  );
});
