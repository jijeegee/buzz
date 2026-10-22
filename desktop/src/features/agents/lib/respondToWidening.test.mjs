import assert from "node:assert/strict";
import test from "node:test";

import {
  isRespondToWidening,
  respondToWideningWarning,
} from "./respondToWidening.ts";

const HEX_A = "a".repeat(64);
const HEX_B = "b".repeat(64);

const ownerOnly = { respondTo: "owner-only" };
const unset = { respondTo: null };
const allowA = { respondTo: "allowlist", respondToAllowlist: [HEX_A] };
const allowAB = { respondTo: "allowlist", respondToAllowlist: [HEX_A, HEX_B] };
const anyone = { respondTo: "anyone" };

test("owner-only to allowlist or anyone widens", () => {
  assert.equal(isRespondToWidening(ownerOnly, allowA), true);
  assert.equal(isRespondToWidening(ownerOnly, anyone), true);
  assert.equal(isRespondToWidening(unset, anyone), true);
});

test("allowlist to anyone widens", () => {
  assert.equal(isRespondToWidening(allowA, anyone), true);
});

test("an allowlist gaining pubkeys widens; same or fewer does not", () => {
  assert.equal(isRespondToWidening(allowA, allowAB), true);
  assert.equal(isRespondToWidening(allowAB, allowA), false);
  assert.equal(
    isRespondToWidening(allowA, {
      respondTo: "allowlist",
      respondToAllowlist: ["A".repeat(64)],
    }),
    false,
    "pubkey comparison is case-insensitive",
  );
});

test("narrowing or unchanged access does not widen", () => {
  assert.equal(isRespondToWidening(anyone, ownerOnly), false);
  assert.equal(isRespondToWidening(anyone, allowA), false);
  assert.equal(isRespondToWidening(allowA, ownerOnly), false);
  assert.equal(isRespondToWidening(anyone, anyone), false);
  assert.equal(isRespondToWidening(ownerOnly, unset), false);
  assert.equal(isRespondToWidening(unset, {}), false);
});

test("a new-agent draft (no saved definition) widens on any non-owner mode", () => {
  assert.equal(isRespondToWidening(null, {}), false);
  assert.equal(isRespondToWidening(null, ownerOnly), false);
  assert.equal(isRespondToWidening(null, allowA), true);
  assert.equal(isRespondToWidening(null, anyone), true);
});

test("respondToWideningWarning names the old and new access", () => {
  assert.equal(
    respondToWideningWarning(ownerOnly, anyone),
    "This draft lets more people command this agent: Only me → Anyone. Review before saving.",
  );
  assert.match(
    respondToWideningWarning(allowA, allowAB),
    /Selected people \(new people added\)/,
  );
  assert.equal(respondToWideningWarning(anyone, ownerOnly), null);
});
