import type { ControlResultFrame } from "@/shared/api/types";
import { type ContextReading, parseContextReading } from "./contextUsageQuery";

/** Terminal outcome of one `compact_session` request, as seen by Desktop. */
export type CompactSessionOutcome =
  | "completed"
  | "failed"
  | "timeout"
  | "busy"
  | "no_session"
  | "unsupported"
  | "ambiguous_target"
  /**
   * The session changed since the reading the user confirmed (new session or
   * usage moved past the harness threshold); nothing was compacted.
   */
  | "stale"
  /** No harness acknowledgement arrived within the ack window. */
  | "unconfirmed"
  /** The harness acknowledged, but no terminal result arrived in time. */
  | "unconfirmed_completion";

const HARNESS_TERMINAL_STATUSES = new Set<string>([
  "completed",
  "failed",
  "timeout",
  "busy",
  "no_session",
  "unsupported",
  "ambiguous_target",
  "stale",
]);

export type CompactTimeoutPhase = "ack" | "completion";

/**
 * Send a `compact_session` control and resolve with its correlated outcome.
 *
 * Correlates on control type, request id, and channel. A `started` ack moves
 * the request into its completion phase (reported through `onStarted`); any
 * harness terminal status settles it. A `stale` result hands the harness's
 * current reading to `onStale` before settling. Missing results resolve to an
 * explicit unconfirmed outcome rather than success. Transport failures reject.
 */
export async function awaitCompactSessionOutcome({
  requestId,
  channelId,
  subscribe,
  send,
  scheduleTimeout,
  onStarted,
  onStale,
}: {
  requestId: string;
  channelId: string;
  subscribe: (listener: (frame: ControlResultFrame) => void) => () => void;
  send: () => Promise<void>;
  scheduleTimeout: (
    phase: CompactTimeoutPhase,
    onTimeout: () => void,
  ) => () => void;
  onStarted?: () => void;
  onStale?: (reading: ContextReading | null) => void;
}): Promise<CompactSessionOutcome> {
  let settled = false;
  let started = false;
  let unsubscribe = () => {};
  let cancelTimeout = () => {};
  let resolveResult: (outcome: CompactSessionOutcome) => void = () => {};
  let rejectResult: (error: unknown) => void = () => {};
  const result = new Promise<CompactSessionOutcome>((resolve, reject) => {
    resolveResult = resolve;
    rejectResult = reject;
  });
  const cleanup = () => {
    unsubscribe();
    cancelTimeout();
  };
  const settle = (outcome: CompactSessionOutcome) => {
    if (settled) return;
    settled = true;
    cleanup();
    resolveResult(outcome);
  };
  const fail = (error: unknown) => {
    if (settled) return;
    settled = true;
    cleanup();
    rejectResult(error);
  };

  unsubscribe = subscribe((frame) => {
    if (
      settled ||
      frame.type !== "compact_session" ||
      frame.requestId !== requestId ||
      frame.channelId !== channelId
    ) {
      return;
    }
    if (frame.status === "started") {
      if (started) return;
      started = true;
      cancelTimeout();
      cancelTimeout = scheduleTimeout("completion", () =>
        settle("unconfirmed_completion"),
      );
      onStarted?.();
      return;
    }
    if (HARNESS_TERMINAL_STATUSES.has(frame.status)) {
      if (frame.status === "stale") {
        onStale?.(parseContextReading(frame.reading));
      }
      settle(frame.status as CompactSessionOutcome);
    }
  });
  // Arm the ack timeout before sending so a hung transport still settles.
  cancelTimeout = scheduleTimeout("ack", () => settle("unconfirmed"));
  try {
    void Promise.resolve(send()).catch(fail);
  } catch (error) {
    fail(error);
  }

  return result;
}

export type CompactOutcomeNotice = {
  tone: "success" | "info" | "error";
  message: string;
};

/** User-facing copy for each compaction outcome. */
export function compactSessionOutcomeNotice(
  outcome: CompactSessionOutcome,
  agentName: string,
): CompactOutcomeNotice {
  switch (outcome) {
    case "completed":
      return {
        tone: "success",
        message: `${agentName}'s session context was compacted.`,
      };
    case "failed":
      return {
        tone: "error",
        message: `${agentName} couldn't compact this session.`,
      };
    case "timeout":
      return {
        tone: "error",
        message: `Compaction timed out before ${agentName} finished.`,
      };
    case "busy":
      return {
        tone: "info",
        message: `${agentName} is working in this session. Try again when the turn finishes.`,
      };
    case "no_session":
      return {
        tone: "info",
        message: `${agentName} has no live session here to compact. It may have restarted since this reading.`,
      };
    case "unsupported":
      return {
        tone: "info",
        message: `${agentName}'s runtime doesn't support compaction.`,
      };
    case "ambiguous_target":
      return {
        tone: "error",
        message: `${agentName} couldn't tell which session to compact.`,
      };
    case "stale":
      return {
        tone: "info",
        message: `${agentName}'s context changed since you looked, so nothing was compacted. Re-check the reading and try again.`,
      };
    case "unconfirmed":
      return {
        tone: "info",
        message: `Compaction requested, but ${agentName} hasn't confirmed it.`,
      };
    case "unconfirmed_completion":
      return {
        tone: "info",
        message: `${agentName} started compacting but hasn't reported a result yet.`,
      };
  }
}
