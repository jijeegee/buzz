import assert from "node:assert/strict";
import test from "node:test";

import {
  NO_DEFAULT_AI_HINT,
  findDefaultAi,
  isDefaultAiEligible,
  resolveAddDefaultAi,
  routingJoinsNewChannels,
} from "./defaultAi.ts";

const KEYED = "ab".repeat(32);
const OTHER = "cd".repeat(32);

test("only Host and Lead join new channels; Off, Smart routing, and unknown do not", () => {
  assert.equal(routingJoinsNewChannels("host"), true);
  assert.equal(routingJoinsNewChannels("lead"), true);
  assert.equal(routingJoinsNewChannels("off"), false);
  assert.equal(routingJoinsNewChannels("desktop-router"), false);
  assert.equal(routingJoinsNewChannels(undefined), false);
  assert.equal(routingJoinsNewChannels(null), false);
});

test("addDefaultAi is true only for a star × a joining mode × the switch on", () => {
  const starred = { pubkey: KEYED, isDefaultAi: true };
  for (const mode of ["off", "host", "lead", "desktop-router", undefined]) {
    for (const defaultAi of [starred, null, undefined]) {
      for (const requested of [true, false]) {
        const expected =
          Boolean(defaultAi) &&
          requested &&
          (mode === "host" || mode === "lead");
        assert.equal(
          resolveAddDefaultAi(defaultAi, requested, mode),
          expected,
          `mode=${mode} star=${Boolean(defaultAi)} requested=${requested}`,
        );
      }
    }
  }
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
