import assert from "node:assert/strict";
import test from "node:test";

import { KUZZ_ID_RULES, kuzzIdError, normalizeKuzzId } from "./kuzzId.ts";

test("a Kuzz ID is saved trimmed, without @, in lowercase", () => {
  assert.equal(normalizeKuzzId("  @Jiho_1 "), "jiho_1");
  assert.equal(kuzzIdError("  @Jiho_1 "), null);
});

test("an empty Kuzz ID clears it", () => {
  assert.equal(kuzzIdError("   "), null);
  assert.equal(normalizeKuzzId("@"), "");
});

test("IDs outside 3-32 of [a-z0-9_] are refused with the rule", () => {
  for (const bad of ["ab", "a".repeat(33), "ji-ho", "ji ho", "지호abc"]) {
    assert.equal(kuzzIdError(bad), KUZZ_ID_RULES, bad);
  }
  assert.equal(kuzzIdError("a".repeat(32)), null);
});
