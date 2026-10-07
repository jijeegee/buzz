/**
 * Pure geometry and staging for the context-window clock gauge.
 *
 * The dial fills clockwise from 12 o'clock in proportion to used / size. The
 * filled wedge is tinted by stage: a subtle neutral tint below the warning
 * threshold, yellow from the warning threshold, red from the critical one.
 */

export type ContextGaugeStage = "normal" | "warning" | "critical";

/** Fractions of the window (0..1) at which the gauge changes stage. */
export type ContextGaugeThresholds = {
  warning: number;
  critical: number;
};

export const DEFAULT_CONTEXT_GAUGE_THRESHOLDS: ContextGaugeThresholds = {
  warning: 0.5,
  critical: 0.8,
};

/** used / size clamped to [0, 1]; 0 for an unusable window. */
export function contextFillFraction(used: number, size: number): number {
  if (!Number.isFinite(used) || !Number.isFinite(size) || size <= 0) return 0;
  return Math.min(1, Math.max(0, used / size));
}

/** Stage for a fill fraction; each threshold is inclusive. */
export function contextGaugeStage(
  fraction: number,
  thresholds: ContextGaugeThresholds = DEFAULT_CONTEXT_GAUGE_THRESHOLDS,
): ContextGaugeStage {
  if (fraction >= thresholds.critical) return "critical";
  if (fraction >= thresholds.warning) return "warning";
  return "normal";
}

/** Whole-percent label value for a fill fraction. */
export function contextFillPercent(fraction: number): number {
  return Math.round(contextFillFraction(fraction, 1) * 100);
}

export type ContextGaugeWedge =
  | { kind: "empty" }
  | { kind: "full" }
  | { kind: "wedge"; path: string };

// A sweep this close to a whole turn cannot be drawn as one SVG arc (start and
// end points coincide), so it renders as a full disc instead.
const FULL_EPSILON = 1e-4;

function round(value: number): number {
  return Math.round(value * 1000) / 1000;
}

/**
 * SVG wedge for `fraction` of a dial centred at (`center`, `center`) with
 * radius `radius`: from 12 o'clock, clockwise.
 */
export function contextGaugeWedge(
  fraction: number,
  center: number,
  radius: number,
): ContextGaugeWedge {
  const clamped = contextFillFraction(fraction, 1);
  if (clamped <= 0) return { kind: "empty" };
  if (clamped >= 1 - FULL_EPSILON) return { kind: "full" };
  const angle = clamped * 2 * Math.PI;
  const endX = round(center + radius * Math.sin(angle));
  const endY = round(center - radius * Math.cos(angle));
  const largeArc = clamped > 0.5 ? 1 : 0;
  const top = round(center - radius);
  return {
    kind: "wedge",
    path: `M ${center} ${center} L ${center} ${top} A ${radius} ${radius} 0 ${largeArc} 1 ${endX} ${endY} Z`,
  };
}

const tokenFormatter = new Intl.NumberFormat("en-US");

/** "50,000 / 200,000 tokens (25%)". */
export function formatContextUsageLabel(used: number, size: number): string {
  const percent = contextFillPercent(contextFillFraction(used, size));
  return `${tokenFormatter.format(used)} / ${tokenFormatter.format(size)} tokens (${percent}%)`;
}

/** Coarse relative age, e.g. "just now", "5m ago", "3h ago", "2d ago". */
export function formatContextUpdatedAgo(
  updatedAt: number,
  now: number = Date.now(),
): string {
  const seconds = Math.max(0, Math.floor((now - updatedAt) / 1000));
  if (seconds < 60) return "just now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86_400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86_400)}d ago`;
}
