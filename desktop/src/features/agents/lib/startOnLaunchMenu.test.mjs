import assert from "node:assert/strict";
import test from "node:test";

import { startOnLaunchMenuState } from "./startOnLaunchMenu.ts";

// Table over the full input space: instance presence × backend × the stored
// flag. The flag must never change the menu state — it only drives `checked`.
const LOCAL = { type: "local" };
const PROVIDER = { type: "provider", id: "blox", config: {} };

const cases = [
  { agent: undefined, expected: "hidden" },
  { agent: { backend: LOCAL, startOnAppLaunch: true }, expected: "toggle" },
  { agent: { backend: LOCAL, startOnAppLaunch: false }, expected: "toggle" },
  {
    agent: { backend: PROVIDER, startOnAppLaunch: true },
    expected: "provider-managed",
  },
  {
    agent: { backend: PROVIDER, startOnAppLaunch: false },
    expected: "provider-managed",
  },
];

for (const { agent, expected } of cases) {
  const label =
    agent === undefined
      ? "no instance"
      : `${agent.backend.type} backend, startOnAppLaunch=${agent.startOnAppLaunch}`;
  test(`startOnLaunchMenuState(${label}) → ${expected}`, () => {
    assert.equal(startOnLaunchMenuState(agent), expected);
  });
}
