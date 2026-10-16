import assert from "node:assert/strict";
import test from "node:test";

import { matchesProjectsSearch } from "./projectsSearch.ts";

test("matches every case-insensitive token across fields", () => {
  assert.equal(
    matchesProjectsSearch("kuzz mobile", [
      "Kuzz Platform",
      "Desktop, relay, and mobile clients",
    ]),
    true,
  );
  assert.equal(
    matchesProjectsSearch("kuzz missing", [
      "Kuzz Platform",
      "Desktop, relay, and mobile clients",
    ]),
    false,
  );
});

test("empty search matches everything", () => {
  assert.equal(matchesProjectsSearch("   ", []), true);
});
