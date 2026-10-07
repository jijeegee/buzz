import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  awaitContextUsageQuery,
  compactDialogView,
  compactUnavailableReason,
  parseContextReading,
} from "./contextUsageQuery.ts";

const REQUEST = "q-1";
const CHANNEL = "chan-1";
const READING = {
  sessionId: "live",
  used: 90_000,
  size: 200_000,
  compactSupported: true,
  updatedAt: "2026-10-07T00:00:00+00:00",
};

function harness() {
  const listeners = new Set();
  const timeouts = [];
  return {
    emit(frame) {
      for (const listener of [...listeners]) listener(frame);
    },
    listenerCount: () => listeners.size,
    timeouts,
    options: {
      subscribe(listener) {
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
      scheduleTimeout(onTimeout) {
        const entry = { onTimeout, cancelled: false };
        timeouts.push(entry);
        return () => {
          entry.cancelled = true;
        };
      },
    },
  };
}

function frame(status, overrides = {}) {
  return {
    type: "query_context_usage",
    status,
    requestId: REQUEST,
    channelId: CHANNEL,
    threadRootEventId: null,
    reading: null,
    ...overrides,
  };
}

function run(h, send = async () => {}) {
  return awaitContextUsageQuery({
    requestId: REQUEST,
    channelId: CHANNEL,
    send,
    ...h.options,
  });
}

describe("parseContextReading", () => {
  it("parses a harness reading", () => {
    assert.deepEqual(parseContextReading(READING), {
      sessionId: "live",
      used: 90_000,
      size: 200_000,
      compactSupported: true,
      updatedAt: Date.parse("2026-10-07T00:00:00+00:00"),
    });
  });

  it("rejects malformed readings", () => {
    for (const value of [
      null,
      "x",
      { used: 1 },
      { used: 1, size: 0 },
      { used: -1, size: 10 },
      { used: "1", size: 10 },
    ]) {
      assert.equal(parseContextReading(value), null, JSON.stringify(value));
    }
  });
});

describe("awaitContextUsageQuery", () => {
  it("resolves ok with the reading and cleans up", async () => {
    const h = harness();
    const result = run(h);
    h.emit(frame("ok", { reading: READING }));
    const settled = await result;
    assert.equal(settled.status, "ok");
    assert.equal(settled.reading.used, 90_000);
    assert.equal(h.timeouts[0].cancelled, true);
    assert.equal(h.listenerCount(), 0);
  });

  it("reads an ok answer with a malformed reading as no_reading", async () => {
    const h = harness();
    const result = run(h);
    h.emit(frame("ok", { reading: { used: 1 } }));
    assert.deepEqual(await result, { status: "no_reading" });
  });

  for (const status of ["no_reading", "busy", "no_session"]) {
    it(`settles on ${status}`, async () => {
      const h = harness();
      const result = run(h);
      h.emit(frame(status));
      assert.deepEqual(await result, { status });
    });
  }

  it("ignores frames for other controls, requests, channels, or statuses", async () => {
    const h = harness();
    const result = run(h);
    h.emit(frame("ok", { type: "compact_session", reading: READING }));
    h.emit(frame("ok", { requestId: "q-other", reading: READING }));
    h.emit(frame("ok", { channelId: "chan-other", reading: READING }));
    h.emit(frame("started"));
    h.emit(frame("no_session"));
    assert.deepEqual(await result, { status: "no_session" });
  });

  it("reports unconfirmed when the harness never answers", async () => {
    const h = harness();
    const result = run(h);
    h.timeouts[0].onTimeout();
    assert.deepEqual(await result, { status: "unconfirmed" });
    assert.equal(h.listenerCount(), 0);
    // A late answer after the timeout changes nothing.
    h.emit(frame("ok", { reading: READING }));
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
});

describe("compactDialogView", () => {
  const cached = {
    sessionId: "cached-session",
    used: 50_000,
    size: 200_000,
    compactSupported: true,
    updatedAt: 1_000,
  };
  const live = parseContextReading(READING);

  it("shows the cached reading and holds Compact while checking", () => {
    const view = compactDialogView({
      cached,
      query: { status: "checking" },
      changed: null,
    });
    assert.equal(view.freshness, "checking");
    assert.equal(view.checking, true);
    assert.equal(view.reading.used, 50_000);
    assert.match(view.note, /Checking/);
  });

  it("shows the live reading once the harness answers", () => {
    const view = compactDialogView({
      cached,
      query: { status: "ok", reading: live },
      changed: null,
    });
    assert.equal(view.freshness, "live");
    assert.equal(view.checking, false);
    assert.equal(view.noSession, false);
    assert.deepEqual(view.reading, {
      sessionId: "live",
      used: 90_000,
      size: 200_000,
      compactSupported: true,
    });
    assert.equal(view.updatedAt, live.updatedAt);
  });

  it("marks the cached reading possibly stale when unconfirmed, still allowing Compact", () => {
    const view = compactDialogView({
      cached,
      query: { status: "unconfirmed" },
      changed: null,
    });
    assert.equal(view.freshness, "possibly_stale");
    assert.equal(view.checking, false);
    assert.equal(view.noSession, false);
    assert.equal(view.reading.sessionId, "cached-session");
    assert.match(view.note, /out of date/);
  });

  it("flags a missing session so Compact is disabled", () => {
    const view = compactDialogView({
      cached,
      query: { status: "no_session" },
      changed: null,
    });
    assert.equal(view.freshness, "no_session");
    assert.equal(view.noSession, true);
    assert.equal(
      compactUnavailableReason({
        compactSupported: view.reading.compactSupported,
        hasActiveTurn: false,
        noSession: view.noSession,
        pending: false,
      }),
      "There is no live session to compact.",
    );
  });

  for (const status of ["busy", "no_reading"]) {
    it(`keeps the cached reading with a note when ${status}`, () => {
      const view = compactDialogView({
        cached,
        query: { status },
        changed: null,
      });
      assert.equal(view.freshness, "cached");
      assert.equal(view.noSession, false);
      assert.equal(view.reading.used, 50_000);
      assert.ok(view.note);
    });
  }

  it("prefers the reading from a stale compact refusal", () => {
    const changed = { ...live, used: 150_000, updatedAt: null };
    const view = compactDialogView({
      cached,
      query: { status: "ok", reading: live },
      changed,
      now: 9_000,
    });
    assert.equal(view.freshness, "changed");
    assert.equal(view.reading.used, 150_000);
    assert.equal(view.updatedAt, 9_000);
    assert.match(view.note, /changed since you looked/);
  });
});

describe("compactUnavailableReason", () => {
  const ready = {
    compactSupported: true,
    hasActiveTurn: false,
    noSession: false,
    pending: false,
  };

  it("allows Compact when nothing blocks it", () => {
    assert.equal(compactUnavailableReason(ready), null);
  });

  it("names the first blocking reason", () => {
    assert.match(
      compactUnavailableReason({ ...ready, noSession: true, pending: true }),
      /no live session/,
    );
    assert.match(
      compactUnavailableReason({ ...ready, compactSupported: false }),
      /doesn't support/,
    );
    assert.match(
      compactUnavailableReason({ ...ready, pending: true }),
      /already in progress/,
    );
    assert.match(
      compactUnavailableReason({ ...ready, hasActiveTurn: true }),
      /working/,
    );
  });
});
