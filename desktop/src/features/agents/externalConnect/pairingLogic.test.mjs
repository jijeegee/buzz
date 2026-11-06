import assert from "node:assert/strict";
import test from "node:test";

import {
  connectedClaim,
  formatCountdown,
  agentRuntime,
  groupExternalAgents,
  mergeRemembered,
  nestExternalDevices,
  withoutDisconnected,
  parseRemembered,
  pairingDeadline,
  secondsLeft,
  startPairingPoll,
  hermesRows,
} from "./pairingLogic.ts";

const AGENT = "a".repeat(64);
const OTHER_AGENT = "b".repeat(64);

function pairing(overrides = {}) {
  return {
    id: "p1",
    state: "waiting",
    agentPubkey: null,
    deviceName: null,
    deviceId: null,
    createdAt: "2026-10-23T05:30:00Z",
    claimedAt: null,
    expiresAt: "2026-10-23T05:40:00Z",
    ...overrides,
  };
}

const connected = pairing({
  state: "connected",
  agentPubkey: AGENT,
  deviceName: "office-laptop",
  deviceId: "d1",
  claimedAt: "2026-10-23T05:32:00Z",
});

/** A manual clock and timer queue for the poll loop. */
function fakeTimers(start = 0) {
  let now = start;
  let next = 1;
  const timers = new Map();
  return {
    now: () => now,
    schedule(run, ms) {
      const handle = next++;
      timers.set(handle, { at: now + ms, run });
      return handle;
    },
    cancel(handle) {
      timers.delete(handle);
    },
    pending: () => timers.size,
    /** Move the clock and run every timer that is due, in order. */
    async advance(ms) {
      const target = now + ms;
      for (;;) {
        const due = [...timers.entries()]
          .filter(([, timer]) => timer.at <= target)
          .sort((a, b) => a[1].at - b[1].at)[0];
        if (!due) break;
        timers.delete(due[0]);
        now = due[1].at;
        due[1].run();
        // Let the awaited list() settle.
        for (let i = 0; i < 5; i += 1) await Promise.resolve();
      }
      now = target;
    },
  };
}

test("countdown: whole seconds left, formatted as m:ss", () => {
  const at = 1_000_000;
  assert.equal(secondsLeft(at, at - 582_000), 582);
  assert.equal(secondsLeft(at, at - 581_400), 582, "rounds up");
  assert.equal(secondsLeft(at, at + 5_000), 0, "never negative");
  assert.equal(secondsLeft(Number.NaN, at), 0);
  assert.equal(formatCountdown(582), "9:42");
  assert.equal(formatCountdown(600), "10:00");
  assert.equal(formatCountdown(5), "0:05");
  assert.equal(formatCountdown(-3), "0:00");
});

test("countdown: from the received expiresIn, not the relay's clock", () => {
  // Received at 50s on this computer's clock, valid for ten minutes.
  assert.equal(pairingDeadline(50_000, 600), 650_000);
  assert.equal(secondsLeft(pairingDeadline(50_000, 600), 50_000), 600);
  // A missing or broken expiresIn ends the code at once.
  assert.equal(pairingDeadline(50_000, 0), 50_000);
  assert.equal(pairingDeadline(50_000, Number.NaN), 50_000);
});

test("connectedClaim: only this code's connected claim", () => {
  assert.equal(connectedClaim([pairing()], "p1"), null);
  assert.equal(connectedClaim([{ ...connected, id: "p2" }], "p1"), null);
  assert.equal(connectedClaim([connected], "p1"), connected);
});

test("poll: asks every 2 s and reports the first connected claim once", async () => {
  const timers = fakeTimers();
  const answers = [[pairing()], [pairing()], [connected]];
  let calls = 0;
  const seen = [];
  startPairingPoll({
    pairingId: "p1",
    expiresAtMs: 600_000,
    list: async () => answers[Math.min(calls++, answers.length - 1)],
    now: timers.now,
    schedule: timers.schedule,
    cancel: timers.cancel,
    onConnected: (claim) => seen.push(claim),
    onExpired: () => assert.fail("not expired"),
  });
  await timers.advance(1_999);
  assert.equal(calls, 0, "first ask after one interval");
  await timers.advance(1);
  assert.equal(calls, 1);
  await timers.advance(2_000);
  assert.equal(calls, 2);
  await timers.advance(2_000);
  assert.deepEqual(seen, [connected]);
  await timers.advance(10_000);
  assert.equal(calls, 3, "stops after connecting");
  assert.equal(timers.pending(), 0);
});

test("poll: stops at expiry and keeps going after a failed check", async () => {
  const timers = fakeTimers();
  let calls = 0;
  let expired = 0;
  startPairingPoll({
    pairingId: "p1",
    expiresAtMs: 7_000,
    list: async () => {
      calls += 1;
      if (calls === 1) throw new Error("offline");
      return [pairing()];
    },
    now: timers.now,
    schedule: timers.schedule,
    cancel: timers.cancel,
    onConnected: () => assert.fail("never connected"),
    onExpired: () => {
      expired += 1;
    },
  });
  await timers.advance(6_000);
  assert.equal(calls, 3, "a failure does not end the loop");
  await timers.advance(2_000);
  assert.equal(expired, 1);
  assert.equal(calls, 3, "no ask after expiry");
  await timers.advance(10_000);
  assert.equal(expired, 1);
  assert.equal(timers.pending(), 0);
});

test("poll: the relay saying expired ends it too", async () => {
  const timers = fakeTimers();
  let expired = 0;
  startPairingPoll({
    pairingId: "p1",
    expiresAtMs: 600_000,
    list: async () => [pairing({ state: "expired" })],
    now: timers.now,
    schedule: timers.schedule,
    cancel: timers.cancel,
    onConnected: () => assert.fail("never connected"),
    onExpired: () => {
      expired += 1;
    },
  });
  await timers.advance(2_000);
  assert.equal(expired, 1);
  assert.equal(timers.pending(), 0);
});

test("poll: stop (screen closed) cancels the timer and drops a late answer", async () => {
  const timers = fakeTimers();
  let release;
  let calls = 0;
  const stop = startPairingPoll({
    pairingId: "p1",
    expiresAtMs: 600_000,
    list: () => {
      calls += 1;
      return new Promise((resolve) => {
        release = resolve;
      });
    },
    now: timers.now,
    schedule: timers.schedule,
    cancel: timers.cancel,
    onConnected: () => assert.fail("closed before the answer"),
    onExpired: () => assert.fail("closed"),
  });
  await timers.advance(2_000);
  assert.equal(calls, 1);
  stop();
  release([connected]);
  await timers.advance(10_000);
  assert.equal(calls, 1);
  assert.equal(timers.pending(), 0);

  const idle = fakeTimers();
  const stopIdle = startPairingPoll({
    pairingId: "p1",
    expiresAtMs: 600_000,
    list: async () => assert.fail("closed before the first ask"),
    now: idle.now,
    schedule: idle.schedule,
    cancel: idle.cancel,
    onConnected: () => {},
    onExpired: () => {},
  });
  assert.equal(idle.pending(), 1);
  stopIdle();
  assert.equal(idle.pending(), 0);
});

test("groups: external computers only, with known agents, remembered past a day", () => {
  const devices = [
    {
      id: "me",
      name: "akak-PC",
      platform: "windows",
      lastSeenAt: null,
      current: true,
    },
    {
      id: "d1",
      name: "office-laptop",
      platform: "external",
      lastSeenAt: null,
      current: false,
    },
    {
      id: "d2",
      name: "mac-mini",
      platform: "external",
      lastSeenAt: null,
      current: false,
    },
  ];
  const remembered = [
    {
      agentPubkey: OTHER_AGENT,
      deviceId: "d2",
      connectedAt: "2026-10-01T00:00:00Z",
    },
    { agentPubkey: AGENT, deviceId: "d1", connectedAt: null },
  ];
  const groups = groupExternalAgents(devices, [connected], remembered);
  assert.deepEqual(groups, [
    {
      deviceId: "d1",
      deviceName: "office-laptop",
      hostDeviceId: "d1",
      // The relay's claim time fills in a remembered entry without one.
      agents: [{ pubkey: AGENT, connectedAt: "2026-10-23T05:32:00Z" }],
      listed: false,
    },
    {
      deviceId: "d2",
      deviceName: "mac-mini",
      hostDeviceId: "d2",
      agents: [{ pubkey: OTHER_AGENT, connectedAt: "2026-10-01T00:00:00Z" }],
      listed: false,
    },
  ]);
  assert.deepEqual(
    groupExternalAgents(devices.slice(0, 1), [connected], remembered),
    [],
  );
});

test("one computer: an external device goes under its signed-in device", () => {
  const pc = { id: "pc", name: "studio", platform: "desktop" };
  const phone = { id: "phone", name: "phone", platform: "mobile" };
  const linked = {
    id: "x1",
    name: "host",
    platform: "external",
    loginDeviceId: "pc",
    agents: [{ pubkey: AGENT, runtime: null }],
  };
  // Nothing to list under the computer: its Disconnect needs its own row.
  const empty = {
    id: "x4",
    name: "empty",
    platform: "external",
    loginDeviceId: "pc",
  };
  const alone = { id: "x2", name: "server", platform: "external" };
  // Signed out elsewhere: the link names a device this list doesn't have.
  const stale = {
    id: "x3",
    name: "old",
    platform: "external",
    loginDeviceId: "gone",
  };
  const devices = [pc, phone, linked, alone, stale, empty];
  assert.deepEqual(nestExternalDevices(devices), [
    { device: pc, externals: [linked] },
    { device: phone, externals: [] },
    { device: alone, externals: [alone] },
    { device: stale, externals: [stale] },
    { device: empty, externals: [empty] },
  ]);
  const [group] = groupExternalAgents(devices, [], []);
  assert.equal(group.deviceId, "x1");
  assert.equal(group.deviceName, "studio");
  assert.equal(group.hostDeviceId, "pc");
});

test("agents: the relay's list decides, remembered entries add the time", () => {
  const device = {
    id: "d1",
    name: "pc",
    platform: "external",
    agents: [{ pubkey: AGENT, runtime: "Hermes" }],
  };
  // OTHER_AGENT was disconnected here: this app still remembers it.
  const remembered = [
    { agentPubkey: AGENT, deviceId: "d1", connectedAt: "2026-10-23T05:32:00Z" },
    { agentPubkey: OTHER_AGENT, deviceId: "d1", connectedAt: null },
  ];
  assert.deepEqual(groupExternalAgents([device], [], remembered)[0].agents, [
    { pubkey: AGENT, connectedAt: "2026-10-23T05:32:00Z" },
  ]);
  assert.deepEqual(groupExternalAgents([device], [], [])[0].agents, [
    { pubkey: AGENT, connectedAt: null },
  ]);
  assert.equal(groupExternalAgents([device], [], [])[0].listed, true);
  assert.deepEqual(withoutDisconnected(remembered, [device]), [remembered[0]]);
  // A device the relay lists nobody on says nothing about them.
  assert.deepEqual(
    withoutDisconnected(remembered, [{ ...device, agents: [] }]),
    remembered,
  );
});

test("runtime: the relay's, else Hermes when this app connected it", () => {
  const hermes = new Set([AGENT]);
  const agent = (runtime) => ({ pubkey: AGENT, runtime });
  assert.equal(agentRuntime(agent("Other"), hermes), "Other");
  assert.equal(agentRuntime(agent(null), hermes), "Hermes");
  assert.equal(agentRuntime(agent(null), new Set()), null);
  assert.equal(agentRuntime(agent("  "), new Set()), null);
});

test("remembered: merged once per agent and computer, disconnected ones dropped", () => {
  const merged = mergeRemembered([], [connected, pairing({ id: "p2" })]);
  assert.deepEqual(merged, [
    { agentPubkey: AGENT, deviceId: "d1", connectedAt: "2026-10-23T05:32:00Z" },
  ]);
  assert.deepEqual(mergeRemembered(merged, [connected]), merged);
  assert.deepEqual(mergeRemembered(merged, [], new Set(["d9"])), []);
  const raw = JSON.stringify([
    ...merged,
    { agentPubkey: "nope", deviceId: "d1" },
    { agentPubkey: OTHER_AGENT, deviceId: "" },
    null,
  ]);
  assert.deepEqual(parseRemembered(raw), merged);
  assert.deepEqual(parseRemembered("{"), []);
  assert.deepEqual(parseRemembered(null), []);
});

test("remembered: a Hermes card connection stays marked Hermes", () => {
  const hermes = {
    agentPubkey: AGENT,
    deviceId: "d1",
    connectedAt: "2026-10-23T05:32:00Z",
    hermes: true,
  };
  // The relay's list (unmarked) does not drop the mark.
  assert.deepEqual(mergeRemembered([hermes], [connected]), [hermes]);
  assert.deepEqual(parseRemembered(JSON.stringify([hermes])), [hermes]);
  // Only `true` marks it.
  assert.deepEqual(
    parseRemembered(JSON.stringify([{ ...hermes, hermes: "yes" }])),
    [
      {
        agentPubkey: AGENT,
        deviceId: "d1",
        connectedAt: "2026-10-23T05:32:00Z",
      },
    ],
  );
  // A disconnected Hermes computer is forgotten with it.
  assert.deepEqual(mergeRemembered([hermes], [], new Set(["d9"])), []);
});

const hermesProfile = (name, fields = {}) => ({
  name,
  displayName: name,
  isDefault: name === "default",
  pairedAgent: null,
  owner: null,
  blockedReason: null,
  gateway: null,
  settling: false,
  ...fields,
});
const hermesStates = (profiles, owner, connected = [], device = false) =>
  hermesRows(profiles, owner, new Set(connected), device).map(
    (row) => row.state,
  );

test("hermes rows: a profile is connected when its pinned agent is", () => {
  const pinned = { pairedAgent: "a1", owner: "me" };
  assert.deepEqual(
    hermesStates([hermesProfile("work", pinned)], "me", ["a1"]),
    ["connected"],
  );
  // Disconnected since: connect again.
  assert.deepEqual(hermesStates([hermesProfile("work", pinned)], "me"), [
    "connect",
  ]);
});

test("hermes rows: paired alone is not connected", () => {
  const row = (fields) =>
    hermesStates(
      [hermesProfile("work", { pairedAgent: "a1", owner: "me", ...fields })],
      "me",
      ["a1"],
    )[0];
  const gateway = (fields) => ({
    running: true,
    kuzz: null,
    error: null,
    busy: false,
    ...fields,
  });
  // A gateway that doesn't report on Kuzz: as before.
  assert.equal(row({}), "connected");
  assert.equal(row({ gateway: gateway({ kuzz: "connected" }) }), "connected");
  // The gateway hasn't loaded the plugin, stopped, or failed.
  assert.equal(row({ gateway: gateway({}) }), "offline");
  assert.equal(row({ gateway: gateway({ running: false }) }), "offline");
  assert.equal(row({ gateway: gateway({ kuzz: "fatal" }) }), "offline");
  // A crash leaves the old record behind.
  assert.equal(
    row({ gateway: gateway({ running: false, kuzz: "connected" }) }),
    "offline",
  );
  // Still coming up, with no restart of ours to wait for: Restart stays open.
  assert.equal(row({ gateway: gateway({ kuzz: "connecting" }) }), "offline");
  assert.equal(row({ settling: true }), "settling");
  // The old record still says connected while a restart runs.
  assert.equal(
    row({ settling: true, gateway: gateway({ kuzz: "connected" }) }),
    "settling",
  );
  assert.equal(
    row({ settling: true, gateway: gateway({ busy: true }) }),
    "busy",
  );
});

test("hermes rows: a default profile from before pins counts as connected", () => {
  const old = [hermesProfile("default", { owner: "me" })];
  assert.deepEqual(hermesStates(old, "me", [], true), ["connected"]);
  // Disconnected since: it can connect again.
  assert.deepEqual(hermesStates(old, "me"), ["connect"]);
  assert.deepEqual(
    hermesStates([hermesProfile("work", { owner: "me" })], "me"),
    ["connect"],
  );
});

test("hermes rows: another account's profile says so", () => {
  assert.deepEqual(
    hermesStates([hermesProfile("work", { owner: "other" })], "me"),
    ["otherAccount"],
  );
});

test("hermes rows: a pinned profile Hermes won't serve is blocked", () => {
  assert.deepEqual(
    hermesStates(
      [hermesProfile("work", { pairedAgent: "a", blockedReason: "no" })],
      "me",
      ["a"],
    ),
    ["blocked"],
  );
});

test("hermes rows: a copied profile with the same pin can connect", () => {
  const pinned = { pairedAgent: "a1", owner: "me" };
  assert.deepEqual(
    hermesStates(
      [hermesProfile("one", pinned), hermesProfile("two", pinned)],
      "me",
      ["a1"],
    ),
    ["connected", "connect"],
  );
});

test("hermes rows: blocked profiles show Hermes's reason, not Connect", () => {
  assert.deepEqual(
    hermesStates(
      [hermesProfile("work", { blockedReason: "same token" })],
      "me",
    ),
    ["blocked"],
  );
});
