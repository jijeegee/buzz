import assert from "node:assert/strict";
import test from "node:test";

import { isPairedAgent } from "./pairedAgent.ts";

const devices = [
  { agents: null },
  { agents: [{ pubkey: "ABC" }, { pubkey: "def" }] },
];

test("a paired agent is found on any computer, in any case", () => {
  assert.equal(isPairedAgent(devices, "abc"), true);
  assert.equal(isPairedAgent(devices, "DEF"), true);
});

test("no device list, or a key that isn't on it, is not paired", () => {
  assert.equal(isPairedAgent(devices, "fff"), false);
  assert.equal(isPairedAgent(undefined, "abc"), false);
});
