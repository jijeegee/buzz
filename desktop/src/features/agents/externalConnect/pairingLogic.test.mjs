import assert from "node:assert/strict";
import test from "node:test";

import {
  connectedClaim,
  formatCountdown,
  groupExternalAgents,
  mergeRemembered,
  parseRemembered,
  secondsLeft,
  startPairingPoll,
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
  const expires = "2026-10-23T05:40:00Z";
  const at = Date.parse(expires);
  assert.equal(secondsLeft(expires, at - 582_000), 582);
  assert.equal(secondsLeft(expires, at - 581_400), 582, "rounds up");
  assert.equal(secondsLeft(expires, at + 5_000), 0, "never negative");
  assert.equal(secondsLeft("not a time", at), 0);
  assert.equal(formatCountdown(582), "9:42");
  assert.equal(formatCountdown(600), "10:00");
  assert.equal(formatCountdown(5), "0:05");
  assert.equal(formatCountdown(-3), "0:00");
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
      // The relay's claim time fills in a remembered entry without one.
      agents: [{ pubkey: AGENT, connectedAt: "2026-10-23T05:32:00Z" }],
    },
    {
      deviceId: "d2",
      deviceName: "mac-mini",
      agents: [{ pubkey: OTHER_AGENT, connectedAt: "2026-10-01T00:00:00Z" }],
    },
  ]);
  assert.deepEqual(
    groupExternalAgents(devices.slice(0, 1), [connected], remembered),
    [],
  );
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
