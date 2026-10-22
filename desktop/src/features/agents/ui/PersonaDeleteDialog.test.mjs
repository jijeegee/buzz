import assert from "node:assert/strict";
import test from "node:test";

import { personaDeleteDescription } from "./PersonaDeleteDialog.tsx";

// Deleting a preset deletes only the preset. Agents made from it own a copy
// of its settings, so the confirmation must never claim they are deleted.

const persona = { displayName: "Scout" };

test("agents made from the preset are disclosed as not affected", () => {
  for (const count of [1, 3]) {
    const copy = personaDeleteDescription(persona, count);
    assert.equal(
      copy,
      "Delete Scout. Agents made from this preset are not affected.",
    );
    assert.doesNotMatch(copy, /deletes \d+ agent/i);
  }
});

test("no agents made from the preset keeps the plain copy", () => {
  const copy = personaDeleteDescription(persona, 0);
  assert.equal(copy, "Delete Scout.");
});

test("null persona keeps the generic fallback", () => {
  assert.equal(personaDeleteDescription(null, 2), "Delete this agent.");
});
