import assert from "node:assert/strict";
import test from "node:test";

import {
  defaultAiSelectionFor,
  defaultAiToggleNotice,
  findDefaultAi,
  isDefaultAiEligible,
} from "./defaultAi.ts";

const KEYED = "ab".repeat(32);
const OTHER = "cd".repeat(32);

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

test("starring sends the pubkey and un-starring the current default clears it", () => {
  assert.equal(defaultAiSelectionFor(KEYED, true), KEYED);
  assert.equal(defaultAiSelectionFor(KEYED, false), null);
});

test("the toggle notice names the agent and the new state", () => {
  assert.equal(
    defaultAiToggleNotice("Scout", true),
    "Scout is now your default AI.",
  );
  assert.equal(
    defaultAiToggleNotice("Scout", false),
    "Scout is no longer your default AI.",
  );
});
