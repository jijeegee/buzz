import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  DEFAULT_CONTEXT_GAUGE_THRESHOLDS,
  contextFillFraction,
  contextFillPercent,
  contextGaugeStage,
  contextGaugeWedge,
  formatContextUpdatedAgo,
  formatContextUsageLabel,
} from "./contextGauge.ts";

/** Parse the arc end point and flags out of a wedge path. */
function arcOf(path) {
  const match = path.match(
    /^M (\S+) (\S+) L (\S+) (\S+) A (\S+) (\S+) 0 ([01]) 1 (\S+) (\S+) Z$/,
  );
  assert.ok(match, `unexpected path: ${path}`);
  const [, cx, cy, sx, sy, , , largeArc, ex, ey] = match;
  return {
    center: [Number(cx), Number(cy)],
    start: [Number(sx), Number(sy)],
    largeArc: Number(largeArc),
    end: [Number(ex), Number(ey)],
  };
}

describe("contextGauge", () => {
  it("clamps the fill fraction and rejects unusable windows", () => {
    assert.equal(contextFillFraction(50, 200), 0.25);
    assert.equal(contextFillFraction(300, 200), 1);
    assert.equal(contextFillFraction(-5, 200), 0);
    assert.equal(contextFillFraction(5, 0), 0);
    assert.equal(contextFillFraction(Number.NaN, 200), 0);
    assert.equal(contextFillFraction(5, Number.POSITIVE_INFINITY), 0);
  });

  it("stages by inclusive default thresholds of 50% and 80%", () => {
    assert.deepEqual(DEFAULT_CONTEXT_GAUGE_THRESHOLDS, {
      warning: 0.5,
      critical: 0.8,
    });
    const table = [
      [0, "normal"],
      [0.25, "normal"],
      [0.4999, "normal"],
      [0.5, "warning"],
      [0.7999, "warning"],
      [0.8, "critical"],
      [1, "critical"],
    ];
    for (const [fraction, stage] of table) {
      assert.equal(contextGaugeStage(fraction), stage, `fraction ${fraction}`);
    }
  });

  it("honours custom thresholds", () => {
    const thresholds = { warning: 0.3, critical: 0.6 };
    assert.equal(contextGaugeStage(0.29, thresholds), "normal");
    assert.equal(contextGaugeStage(0.3, thresholds), "warning");
    assert.equal(contextGaugeStage(0.6, thresholds), "critical");
  });

  it("draws nothing when empty and a full disc when full", () => {
    assert.deepEqual(contextGaugeWedge(0, 10, 8), { kind: "empty" });
    assert.deepEqual(contextGaugeWedge(-1, 10, 8), { kind: "empty" });
    assert.deepEqual(contextGaugeWedge(1, 10, 8), { kind: "full" });
    assert.deepEqual(contextGaugeWedge(0.99999, 10, 8), { kind: "full" });
    assert.deepEqual(contextGaugeWedge(2, 10, 8), { kind: "full" });
  });

  it("starts the wedge at 12 o'clock and sweeps clockwise", () => {
    const quarter = contextGaugeWedge(0.25, 10, 8);
    assert.equal(quarter.kind, "wedge");
    const q = arcOf(quarter.path);
    assert.deepEqual(q.center, [10, 10]);
    assert.deepEqual(q.start, [10, 2]); // 12 o'clock
    assert.deepEqual(q.end, [18, 10]); // 3 o'clock
    assert.equal(q.largeArc, 0);

    const half = arcOf(contextGaugeWedge(0.5, 10, 8).path);
    assert.deepEqual(half.end, [10, 18]); // 6 o'clock
    assert.equal(half.largeArc, 0);

    const threeQuarters = arcOf(contextGaugeWedge(0.75, 10, 8).path);
    assert.deepEqual(threeQuarters.end, [2, 10]); // 9 o'clock
    assert.equal(threeQuarters.largeArc, 1);

    // Nearly full lands just short of 12, past 11 o'clock, on the left side.
    const nearlyFull = arcOf(contextGaugeWedge(0.95, 10, 8).path);
    assert.ok(nearlyFull.end[0] < 10);
    assert.ok(nearlyFull.end[1] < 10);
    assert.equal(nearlyFull.largeArc, 1);
  });

  it("formats usage with grouped tokens and a whole percent", () => {
    assert.equal(
      formatContextUsageLabel(50_000, 200_000),
      "50,000 / 200,000 tokens (25%)",
    );
    assert.equal(
      formatContextUsageLabel(199_999, 200_000),
      "199,999 / 200,000 tokens (100%)",
    );
    assert.equal(contextFillPercent(0.004), 0);
    assert.equal(contextFillPercent(0.505), 51);
  });

  it("formats a coarse relative age", () => {
    const now = Date.parse("2026-10-07T12:00:00.000Z");
    assert.equal(formatContextUpdatedAgo(now - 30_000, now), "just now");
    assert.equal(formatContextUpdatedAgo(now + 5_000, now), "just now");
    assert.equal(formatContextUpdatedAgo(now - 5 * 60_000, now), "5m ago");
    assert.equal(formatContextUpdatedAgo(now - 3 * 3_600_000, now), "3h ago");
    assert.equal(formatContextUpdatedAgo(now - 2 * 86_400_000, now), "2d ago");
  });
});
