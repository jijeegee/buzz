import assert from "node:assert/strict";
import test from "node:test";

import {
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
