import assert from "node:assert/strict";
import test from "node:test";

import {
  LEAD_NEEDS_LOCAL_REASON,
  modeNeedsAgent,
  ROUTING_MODE_OPTIONS,
  routingHoldFor,
  routingNameOf,
  routingPickerOptions,
  routingRestartAffordances,
  routingStatusLine,
  routingSummaryText,
  routingTransitionFor,
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

test("every mode ships: Off, Host, Lead, and Smart routing are selectable", () => {
  assert.deepEqual(
    ROUTING_MODE_OPTIONS.map((option) => [option.mode, option.available]),
    [
      ["off", true],
      ["host", true],
      ["lead", true],
      ["desktop-router", true],
    ],
  );
  assert.deepEqual(
    ROUTING_MODE_OPTIONS.map((option) => modeNeedsAgent(option.mode)),
    [false, true, true, false],
  );
});

const LOCAL = { type: "local" };
const REMOTE = { type: "provider", id: "cloud", config: {} };
const PICKER_AGENTS = [
  { pubkey: HONEY, name: "Honey", backend: LOCAL },
  { pubkey: "", name: "Definition", backend: LOCAL },
  { pubkey: FIZZ, name: "Fizz", backend: REMOTE },
];

test("the host picker offers keyed agents of any backend, sorted by name", () => {
  assert.deepEqual(routingPickerOptions("host", PICKER_AGENTS), [
    { pubkey: FIZZ, name: "Fizz", disabledReason: null },
    { pubkey: HONEY, name: "Honey", disabledReason: null },
  ]);
});

test("the lead picker lists remote agents disabled with the reason", () => {
  assert.deepEqual(routingPickerOptions("lead", PICKER_AGENTS), [
    { pubkey: FIZZ, name: "Fizz", disabledReason: LEAD_NEEDS_LOCAL_REASON },
    { pubkey: HONEY, name: "Honey", disabledReason: null },
  ]);
  assert.match(LEAD_NEEDS_LOCAL_REASON, /needs an agent on this computer/);
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
      "Switching — Honey stops hosting after it restarts. Fizz becomes the host after Honey restarts.",
    ],
    [
      // A held gainer that is not running yet is part of the switch.
      status({
        routingAgent: FIZZ,
        applied: { state: "switching" },
        agents: [
          agent({ runningRole: "dispatcher", stale: true }),
          agent({
            pubkey: FIZZ,
            running: false,
            desiredRole: "dispatcher",
            hold: true,
          }),
        ],
      }),
      "switching",
      "Switching — Honey stops hosting after it restarts. Fizz becomes the host after Honey restarts.",
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

test("routingTransitionFor: after a save, the losing host restarts now and the held gainer waits", () => {
  // Star moved Honey → Fizz under Host: Honey still dispatches (stale, loser),
  // Fizz runs plain and is held behind it.
  const switching = status({
    agents: [
      agent({ runningRole: "dispatcher", desiredRole: "none", stale: true }),
      agent({
        pubkey: FIZZ,
        desiredRole: "dispatcher",
        stale: true,
        hold: true,
      }),
    ],
  });
  assert.equal(routingTransitionFor(switching, HONEY), true);
  assert.equal(routingTransitionFor(switching, FIZZ), false);
  // Honey restarted plain: the hold releases and Fizz is promoted now.
  const released = status({
    agents: [
      agent(),
      agent({ pubkey: FIZZ, desiredRole: "dispatcher", stale: true }),
    ],
  });
  assert.equal(routingTransitionFor(released, HONEY), false);
  assert.equal(routingTransitionFor(released, FIZZ), true);
  // A remote agent needs a redeploy; unknown status is no transition.
  const remote = status({
    agents: [agent({ local: false, runningRole: "dispatcher", stale: true })],
  });
  assert.equal(routingTransitionFor(remote, HONEY), false);
  assert.equal(routingTransitionFor(undefined, HONEY), false);
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
