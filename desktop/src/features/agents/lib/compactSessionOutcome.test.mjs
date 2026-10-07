import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  awaitCompactSessionOutcome,
  compactSessionOutcomeNotice,
} from "./compactSessionOutcome.ts";

function harness() {
  const listeners = new Set();
  const timeouts = [];
  let started = 0;
  return {
    emit(frame) {
      for (const listener of [...listeners]) listener(frame);
    },
    listenerCount: () => listeners.size,
    timeouts,
    startedCount: () => started,
    options: {
      subscribe(listener) {
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
      scheduleTimeout(phase, onTimeout) {
        const entry = { phase, onTimeout, cancelled: false };
        timeouts.push(entry);
        return () => {
          entry.cancelled = true;
        };
      },
      onStarted() {
        started += 1;
      },
    },
  };
}

const REQUEST = "req-1";
const CHANNEL = "chan-1";

function frame(status, overrides = {}) {
  return {
    type: "compact_session",
    status,
    requestId: REQUEST,
    channelId: CHANNEL,
    threadRootEventId: null,
    ...overrides,
  };
}

function run(h, send = async () => {}) {
  return awaitCompactSessionOutcome({
    requestId: REQUEST,
    channelId: CHANNEL,
    send,
    ...h.options,
  });
}

describe("awaitCompactSessionOutcome", () => {
  it("moves from started to a terminal completed result", async () => {
    const h = harness();
    const outcome = run(h);
    assert.equal(h.timeouts[0].phase, "ack");
    h.emit(frame("started"));
    assert.equal(h.startedCount(), 1);
    assert.equal(h.timeouts[0].cancelled, true);
    assert.equal(h.timeouts[1].phase, "completion");
    // A replayed ack does not restart the completion phase.
    h.emit(frame("started"));
    assert.equal(h.startedCount(), 1);
    h.emit(frame("completed"));
    assert.equal(await outcome, "completed");
    assert.equal(h.timeouts[1].cancelled, true);
    assert.equal(h.listenerCount(), 0);
  });

  for (const status of [
    "busy",
    "no_session",
    "unsupported",
    "ambiguous_target",
    "failed",
    "timeout",
    "stale",
  ]) {
    it(`settles on terminal ${status}`, async () => {
      const h = harness();
      const outcome = run(h);
      h.emit(frame(status));
      assert.equal(await outcome, status);
    });
  }

  it("hands a stale result's reading to onStale before settling", async () => {
    const h = harness();
    const readings = [];
    const outcome = awaitCompactSessionOutcome({
      requestId: REQUEST,
      channelId: CHANNEL,
      send: async () => {},
      ...h.options,
      onStale: (reading) => readings.push(reading),
    });
    h.emit(
      frame("stale", {
        reading: {
          sessionId: "live",
          used: 150000,
          size: 200000,
          compactSupported: true,
          updatedAt: "2026-10-07T00:00:00+00:00",
        },
      }),
    );
    assert.equal(await outcome, "stale");
    assert.equal(readings.length, 1);
    assert.equal(readings[0].sessionId, "live");
    assert.equal(readings[0].used, 150000);
  });

  it("passes null to onStale when the stale result has no usable reading", async () => {
    const h = harness();
    const readings = [];
    const outcome = awaitCompactSessionOutcome({
      requestId: REQUEST,
      channelId: CHANNEL,
      send: async () => {},
      ...h.options,
      onStale: (reading) => readings.push(reading),
    });
    h.emit(frame("stale"));
    assert.equal(await outcome, "stale");
    assert.deepEqual(readings, [null]);
  });

  it("ignores frames for other controls, requests, or channels", async () => {
    const h = harness();
    const outcome = run(h);
    h.emit(frame("completed", { type: "cancel_turn" }));
    h.emit(frame("completed", { requestId: "req-other" }));
    h.emit(frame("completed", { channelId: "chan-other" }));
    h.emit(frame("weird_status"));
    h.emit(frame("busy"));
    assert.equal(await outcome, "busy");
  });

  it("reports unconfirmed when no ack arrives", async () => {
    const h = harness();
    const outcome = run(h);
    h.timeouts[0].onTimeout();
    assert.equal(await outcome, "unconfirmed");
  });

  it("reports unconfirmed_completion when the terminal result never arrives", async () => {
    const h = harness();
    const outcome = run(h);
    h.emit(frame("started"));
    h.timeouts[1].onTimeout();
    assert.equal(await outcome, "unconfirmed_completion");
  });

  it("rejects on transport failure", async () => {
    const h = harness();
    await assert.rejects(
      run(h, async () => {
        throw new Error("relay down");
      }),
      /relay down/,
    );
    assert.equal(h.listenerCount(), 0);
  });

  it("accepts a result that arrives before the send resolves", async () => {
    const h = harness();
    const outcome = run(h, async () => {
      h.emit(frame("unsupported"));
    });
    assert.equal(await outcome, "unsupported");
  });
});

describe("compactSessionOutcomeNotice", () => {
  it("maps every outcome to a tone and names the agent", () => {
    const expected = {
      completed: "success",
      failed: "error",
      timeout: "error",
      busy: "info",
      no_session: "info",
      unsupported: "info",
      ambiguous_target: "error",
      stale: "info",
      unconfirmed: "info",
      unconfirmed_completion: "info",
    };
    for (const [outcome, tone] of Object.entries(expected)) {
      const notice = compactSessionOutcomeNotice(outcome, "Fizz");
      assert.equal(notice.tone, tone, outcome);
      assert.match(notice.message, /Fizz/, outcome);
    }
  });

  it("tells the user to re-check on stale", () => {
    assert.match(
      compactSessionOutcomeNotice("stale", "Fizz").message,
      /changed since you looked.*Re-check/,
    );
  });
});
