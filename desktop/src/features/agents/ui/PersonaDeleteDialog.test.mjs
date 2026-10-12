import assert from "node:assert/strict";
import test from "node:test";

import { personaDeleteDescription } from "./PersonaDeleteDialog.tsx";

// Regression guard for the persona-cascade consent copy: deleting a persona
// with instances also archives each instance's identity on the relay
// (NIP-IA 9035), a durable externally visible side effect. The confirmation
// dialog must disclose it before the destructive confirm, exactly like the
// direct agent-delete dialog does.

const persona = { displayName: "Scout" };

test("cascade delete discloses member-list removal (plural)", () => {
  const copy = personaDeleteDescription(persona, 3);
  assert.match(copy, /deletes 3 agent instances, so/);
  assert.match(copy, /no longer appear in member lists/);
});

test("cascade delete discloses member-list removal (singular)", () => {
  const copy = personaDeleteDescription(persona, 1);
  assert.match(copy, /deletes 1 agent instance, so/);
  assert.match(copy, /no longer appears in member lists/);
});

test("no instances → no archival claim (nothing is archived)", () => {
  const copy = personaDeleteDescription(persona, 0);
  assert.equal(copy, "Delete Scout.");
  assert.doesNotMatch(copy, /archiv/i);
});

test("null persona keeps the generic fallback", () => {
  assert.equal(personaDeleteDescription(null, 2), "Delete this agent.");
});
