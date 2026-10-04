import assert from "node:assert/strict";
import test from "node:test";

import {
  hostPickerOptions,
  modeNeedsAgent,
  ROUTING_MODE_OPTIONS,
  routingHoldFor,
  routingNameOf,
  routingRestartAffordances,
  routingStatusLine,
  routingSummaryText,
} from "./channelRouting.ts";

const HONEY = "aa".repeat(32);
const FIZZ = "bb".repeat(32);
const nameOf = routingNameOf([
  { pubkey: HONEY, name: "Honey" },
  { pubkey: FIZZ, name: "Fizz" },
]);

function agent(overrides) {
  return {
    pubkey: HONEY,
    running: true,
    local: true,
    runningRole: "none",
    desiredRole: "none",
    stale: false,
    hold: false,
    ...overrides,
  };
}

function status(overrides) {
  return {
    mode: "host",
    routingAgent: HONEY,
    applied: { state: "off" },
    routerActive: false,
    agents: [],
    ...overrides,
  };
}

test("only Off and Host are selectable; Lead and Smart routing render as coming soon", () => {
  assert.deepEqual(
    ROUTING_MODE_OPTIONS.map((option) => [option.mode, option.available]),
    [
      ["off", true],
      ["host", true],
      ["lead", false],
      ["desktop-router", false],
    ],
  );
  assert.deepEqual(
    ROUTING_MODE_OPTIONS.map((option) => modeNeedsAgent(option.mode)),
    [false, true, true, false],
  );
});

test("the host picker offers keyed agents of any backend, sorted by name", () => {
  assert.deepEqual(
    hostPickerOptions([
      { pubkey: HONEY, name: "Honey" },
      { pubkey: "", name: "Definition" },
      { pubkey: FIZZ, name: "Fizz" },
    ]),
    [
      { pubkey: FIZZ, name: "Fizz" },
      { pubkey: HONEY, name: "Honey" },
    ],
  );
});

test("the status line reports what is applied, not what is saved", () => {
  const rows = [
    [
      status({ applied: { state: "hosting", pubkey: HONEY } }),
      "on",
      "On — Honey is hosting.",
    ],
    [
      status({ mode: "off", applied: { state: "off" } }),
      "off",
      "Off — only @mentioned agents answer.",
    ],
    [
      status({ routingAgent: null }),
      "attention",
      "Choose a host agent to turn this on.",
    ],
    [
      status({
        agents: [agent({ running: false, desiredRole: "dispatcher" })],
      }),
      "attention",
      "Honey isn't running. Start it to turn this on.",
    ],
    [
      // Off saved, but Honey still runs as host until it restarts.
      status({
        mode: "off",
        applied: { state: "switching" },
        agents: [agent({ runningRole: "dispatcher", stale: true })],
      }),
      "switching",
      "Switching — Honey stops hosting after it restarts.",
    ],
    [
      status({
        applied: { state: "switching" },
        agents: [agent({ desiredRole: "dispatcher", stale: true })],
      }),
      "switching",
      "Switching — Honey becomes the host after it restarts.",
    ],
    [
      status({
        routingAgent: FIZZ,
        applied: { state: "switching" },
        agents: [
          agent({ runningRole: "dispatcher", stale: true }),
          agent({
            pubkey: FIZZ,
            desiredRole: "dispatcher",
            stale: true,
            hold: true,
          }),
        ],
      }),
      "switching",
      "Switching — Honey stops hosting after it restarts. Fizz becomes the host after it restarts.",
    ],
  ];
  for (const [input, tone, text] of rows) {
    assert.deepEqual(routingStatusLine(input, nameOf), { tone, text });
  }
});

test("Restart now: losing agent first, gaining agent held, mid-turn and remote disabled with reasons", () => {
  const handoff = status({
    routingAgent: FIZZ,
    applied: { state: "switching" },
    agents: [
      agent({ runningRole: "dispatcher", stale: true }),
      agent({
        pubkey: FIZZ,
        desiredRole: "dispatcher",
        stale: true,
        hold: true,
      }),
    ],
  });
  assert.deepEqual(
    routingRestartAffordances(handoff, { isWorking: () => false, nameOf }),
    [
      {
        pubkey: HONEY,
        label: "Restart Honey now",
        disabled: false,
        reason: null,
      },
      {
        pubkey: FIZZ,
        label: "Restart Fizz now",
        disabled: true,
        reason: "Waiting for Honey to stop hosting.",
      },
    ],
  );

  const [busy] = routingRestartAffordances(handoff, {
    isWorking: (pubkey) => pubkey === HONEY,
    nameOf,
  });
  assert.equal(busy.disabled, true);
  assert.match(busy.reason, /middle of a turn/);

  const remote = status({
    mode: "off",
    applied: { state: "switching" },
    agents: [agent({ local: false, runningRole: "dispatcher", stale: true })],
  });
  assert.deepEqual(
    routingRestartAffordances(remote, { isWorking: () => false, nameOf }),
    [
      {
        pubkey: HONEY,
        label: null,
        disabled: true,
        reason: "Redeploy Honey to finish switching.",
      },
    ],
  );

  // Nothing stale, nothing to restart.
  assert.deepEqual(
    routingRestartAffordances(
      status({
        agents: [
          agent({ runningRole: "dispatcher", desiredRole: "dispatcher" }),
        ],
      }),
      { isWorking: () => false, nameOf },
    ),
    [],
  );
});

test("routingHoldFor reads the plan's hold and treats unknown status as not held", () => {
  const held = status({ agents: [agent({ pubkey: FIZZ, hold: true })] });
  assert.equal(routingHoldFor(held, FIZZ), true);
  assert.equal(routingHoldFor(held, HONEY), false);
  assert.equal(routingHoldFor(undefined, FIZZ), false);
});

test("the Settings summary names the saved mode and its agent", () => {
  assert.equal(
    routingSummaryText(status({ mode: "off" }), nameOf),
    "Channel routing: Off",
  );
  assert.equal(
    routingSummaryText(status({}), nameOf),
    "Channel routing: Host (Honey)",
  );
  assert.equal(
    routingSummaryText(status({ routingAgent: null }), nameOf),
    "Channel routing: Host (no agent chosen)",
  );
});
