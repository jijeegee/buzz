import assert from "node:assert/strict";
import test from "node:test";

import { upgradeLegacyAppLinks } from "./legacyAppLinks.ts";

test("rewrites old app links and leaves other text alone", () => {
  assert.equal(
    upgradeLegacyAppLinks("see buzz://message?channel=a&id=b now"), // kuzz-rename: keep
    "see kuzz://message?channel=a&id=b now",
  );
  assert.equal(
    upgradeLegacyAppLinks("[pr](buzz://pr?id=1)"), // kuzz-rename: keep
    "[pr](kuzz://pr?id=1)",
  );
  assert.equal(
    upgradeLegacyAppLinks("https://x.example/buzz://no"), // kuzz-rename: keep
    "https://x.example/buzz://no", // kuzz-rename: keep
  );
  assert.equal(upgradeLegacyAppLinks("plain text"), "plain text");
});
