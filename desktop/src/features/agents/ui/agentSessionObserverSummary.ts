import type { ObserverEvent, TranscriptItem } from "./agentSessionTypes";
import { asRecord, asString } from "./agentSessionUtils";

/** Observer event kind standing in for events folded by a summary tier. */
export const OBSERVER_GAP_KIND = "observer_gap";

// Kinds a summary tier rewrites (and marks with `detail`). Lifecycle kinds are
// never summarised, so they say nothing about the current tier.
const SUMMARISED_KINDS = new Set([
  "acp_read",
  "acp_write",
  "acp_parse_error",
  OBSERVER_GAP_KIND,
]);

/** Whether an event was summarised by the free or standard observer tier. */
export function isSummaryDetail(detail: string | null | undefined): boolean {
  return detail === "free" || detail === "standard";
}

/**
 * Whether the feed is currently a summary view: the latest event a tier could
 * have summarised carries a summary `detail`. Latest wins so an upgrade to
 * full detail mid-session drops the badge.
 */
export function isObserverSummaryView(
  events: readonly Pick<ObserverEvent, "kind" | "detail">[],
): boolean {
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index];
    if (SUMMARISED_KINDS.has(event.kind)) {
      return isSummaryDetail(event.detail);
    }
  }
  return false;
}

/** Argument preview of a summarised tool update (`rawInput.preview`). */
export function extractToolArgsPreview(
  update: Record<string, unknown>,
): string | null {
  return asString(asRecord(update.rawInput).preview);
}

function count(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

/** One-line description of an `observer_gap` payload. */
export function describeObserverGap(payload: unknown): string {
  const record = asRecord(payload);
  const tools = count(record.folded_tools);
  if (tools === 0) {
    const events = count(record.folded_events);
    return `${events} ${events === 1 ? "update" : "updates"} skipped`;
  }
  const names = Array.isArray(record.tool_names)
    ? record.tool_names.filter(
        (name): name is string => typeof name === "string",
      )
    : [];
  const summary = `${tools} ${tools === 1 ? "tool" : "tools"} ran in between`;
  if (names.length === 0) return summary;
  return `${summary} (${names.join(", ")}${tools > names.length ? "…" : ""})`;
}

type ToolTranscriptItem = Extract<TranscriptItem, { type: "tool" }>;

// Status a turn's leftover running tools take when the turn ends.
const TURN_END_TOOL_STATUS: Record<string, "completed" | "failed"> = {
  turn_completed: "completed",
  turn_error: "failed",
};

/**
 * Tools a turn left running, settled by a turn-ending event. Summary tiers can
 * fold or drop a tool's terminal update, so `turn_completed` closes the turn's
 * running tools as completed. buzz-acp sends `turn_error` after
 * `turn_completed`, so an error also re-marks the tools that completion
 * closed — but never a tool that reported its own result.
 */
export function closeTurnTools(
  items: readonly TranscriptItem[],
  event: Pick<ObserverEvent, "kind" | "channelId" | "turnId" | "timestamp">,
): ToolTranscriptItem[] {
  const status = TURN_END_TOOL_STATUS[event.kind];
  if (!status || !event.turnId) return [];
  const closed: ToolTranscriptItem[] = [];
  for (const item of items) {
    if (
      item.type !== "tool" ||
      item.turnId !== event.turnId ||
      (event.channelId != null && item.channelId !== event.channelId)
    ) {
      continue;
    }
    const terminal = item.status === "completed" || item.status === "failed";
    if (terminal && !(status === "failed" && item.closedByTurnEnd)) continue;
    closed.push({
      ...item,
      status,
      closedByTurnEnd: true,
      completedAt: item.completedAt ?? event.timestamp,
    });
  }
  return closed;
}
