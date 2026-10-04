import assert from "node:assert/strict";
import test from "node:test";

import {
  NO_DEFAULT_AI_HINT,
  findDefaultAi,
  isDefaultAiEligible,
  resolveAddDefaultAi,
} from "./defaultAi.ts";

const KEYED = "ab".repeat(32);
const OTHER = "cd".repeat(32);

test("a create form submits addDefaultAi only when an agent is starred and the switch is on", () => {
  const starred = { pubkey: KEYED, isDefaultAi: true };
  assert.equal(resolveAddDefaultAi(starred, true), true);
  assert.equal(resolveAddDefaultAi(starred, false), false);
  // Preference on, but nothing to add: forced false.
  assert.equal(resolveAddDefaultAi(null, true), false);
  assert.equal(resolveAddDefaultAi(undefined, true), false);
});

test("no starred agent yields null", () => {
  assert.equal(findDefaultAi([]), null);
  assert.equal(
    findDefaultAi([
      { pubkey: KEYED, isDefaultAi: false },
      { pubkey: OTHER, isDefaultAi: false },
    ]),
    null,
  );
});

test("the one starred agent is returned as-is", () => {
  const starred = { pubkey: OTHER, isDefaultAi: true, name: "Scout" };
  const found = findDefaultAi([
    { pubkey: KEYED, isDefaultAi: false, name: "Other" },
    starred,
  ]);
  assert.equal(found, starred);
});

test("a key-less record is never eligible and is ignored even if flagged", () => {
  assert.equal(isDefaultAiEligible({ pubkey: "" }), false);
  assert.equal(isDefaultAiEligible({ pubkey: "   " }), false);
  assert.equal(isDefaultAiEligible({ pubkey: KEYED }), true);

  // Rust enforces the gate; the frontend must not surface a corrupted flag
  // on a record that could never have been starred through the command.
  assert.equal(findDefaultAi([{ pubkey: "", isDefaultAi: true }]), null);
});

test("the empty-state hint points at the Channel routing card", () => {
  // The hint must say where the star lives; a bare "none" is a dead end.
  assert.match(NO_DEFAULT_AI_HINT, /Agents › Channel routing/);
});
