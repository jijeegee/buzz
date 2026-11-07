import assert from "node:assert/strict";
import test from "node:test";

import { pickDefaultProjectsAgent } from "./projectAgentSelection.ts";

test("prefers Kuzi over the first running agent", () => {
  const implementationPartner = {
    name: "Implementation Partner",
    personaId: "custom:implementation",
  };
  const kuzi = { name: "Kuzi", personaId: "builtin:kuzi" };
  assert.equal(pickDefaultProjectsAgent([implementationPartner, kuzi]), kuzi);
});

test("ignores an unmanaged agent using the Fizz display name", () => {
  const managed = { name: "Builder", personaId: "custom:builder" };
  const spoofedFizz = { name: "Fizz" };
  assert.equal(pickDefaultProjectsAgent([managed, spoofedFizz]), managed);
  assert.equal(pickDefaultProjectsAgent([managed]), managed);
  assert.equal(pickDefaultProjectsAgent([]), null);
});
