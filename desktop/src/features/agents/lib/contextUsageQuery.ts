import type { AgentContextUsage } from "@/features/agents/agentContextUsageStore";
import type { ControlResultFrame } from "@/shared/api/types";

/**
 * One live context reading reported by the harness in a `control_result`
 * (`query_context_usage` answers and `compact_session` `stale` results).
 */
export type ContextReading = {
  sessionId: string | null;
  used: number;
  size: number;
  compactSupported: boolean;
  /** When the harness last saw a usage update for the session, epoch ms. */
  updatedAt: number | null;
};

function isNonNegativeFinite(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0;
}

/** Parse a harness `reading` object, or null when it is malformed. */
export function parseContextReading(value: unknown): ContextReading | null {
  if (typeof value !== "object" || value === null) return null;
  const record = value as Record<string, unknown>;
  const { used, size } = record;
  if (!isNonNegativeFinite(used) || !isNonNegativeFinite(size) || size <= 0) {
    return null;
  }
  const updatedAt =
    typeof record.updatedAt === "string" ? Date.parse(record.updatedAt) : NaN;
  return {
    sessionId: typeof record.sessionId === "string" ? record.sessionId : null,
    used,
    size,
    compactSupported: record.compactSupported === true,
    updatedAt: Number.isFinite(updatedAt) ? updatedAt : null,
  };
}

/** Outcome of one `query_context_usage` request, as seen by Desktop. */
export type ContextUsageQueryResult =
  | { status: "ok"; reading: ContextReading }
  /** A live session exists but has not reported usage yet. */
  | { status: "no_reading" }
  /** The scope's turn is running; its usage is still changing. */
  | { status: "busy" }
  /** No live session for this scope (e.g. the harness restarted). */
  | { status: "no_session" }
  /** No answer in time: offline harness or one without query support. */
  | { status: "unconfirmed" };

/**
 * Send a `query_context_usage` control and resolve with its correlated
 * answer. Correlates on control type, request id, and channel; a missing
 * answer resolves to `unconfirmed`, never to a reading. Transport failures
 * reject.
 */
export async function awaitContextUsageQuery({
  requestId,
  channelId,
  subscribe,
  send,
  scheduleTimeout,
}: {
  requestId: string;
  channelId: string;
  subscribe: (listener: (frame: ControlResultFrame) => void) => () => void;
  send: () => Promise<void>;
  scheduleTimeout: (onTimeout: () => void) => () => void;
}): Promise<ContextUsageQueryResult> {
  let settled = false;
  let unsubscribe = () => {};
  let cancelTimeout = () => {};
  let resolveResult: (result: ContextUsageQueryResult) => void = () => {};
  let rejectResult: (error: unknown) => void = () => {};
  const result = new Promise<ContextUsageQueryResult>((resolve, reject) => {
    resolveResult = resolve;
    rejectResult = reject;
  });
  const finish = (action: () => void) => {
    if (settled) return;
    settled = true;
    unsubscribe();
    cancelTimeout();
    action();
  };

  unsubscribe = subscribe((frame) => {
    if (
      settled ||
      frame.type !== "query_context_usage" ||
      frame.requestId !== requestId ||
      frame.channelId !== channelId
    ) {
      return;
    }
    switch (frame.status) {
      case "ok": {
        const reading = parseContextReading(frame.reading);
        finish(() =>
          resolveResult(
            reading ? { status: "ok", reading } : { status: "no_reading" },
          ),
        );
        return;
      }
      case "no_reading":
        finish(() => resolveResult({ status: "no_reading" }));
        return;
      case "busy":
        finish(() => resolveResult({ status: "busy" }));
        return;
      case "no_session":
        finish(() => resolveResult({ status: "no_session" }));
        return;
    }
  });
  cancelTimeout = scheduleTimeout(() =>
    finish(() => resolveResult({ status: "unconfirmed" })),
  );
  try {
    void Promise.resolve(send()).catch((error) =>
      finish(() => rejectResult(error)),
    );
  } catch (error) {
    finish(() => rejectResult(error));
  }
  return result;
}

/** Where the Compact dialog's live check stands. */
export type ContextQueryState =
  | { status: "checking" }
  | ContextUsageQueryResult;

/** What the reading shown in the Compact dialog is based on. */
export type CompactDialogFreshness =
  | "checking"
  | "live"
  | "cached"
  | "possibly_stale"
  | "no_session"
  | "changed";

export type CompactDialogView = {
  /** Reading to display, and to send as the compact expectation. */
  reading: Pick<
    ContextReading,
    "sessionId" | "used" | "size" | "compactSupported"
  >;
  /** Reading time to display, epoch ms. */
  updatedAt: number;
  freshness: CompactDialogFreshness;
  /** Status line under the reading, or null. */
  note: string | null;
  /** Live check still running: hold Compact until it answers or times out. */
  checking: boolean;
  /** The harness has no live session for this scope: nothing to compact. */
  noSession: boolean;
};

/**
 * Derive the Compact dialog's displayed reading and status from the cached
 * reading, the live query, and a `stale` compact answer (which wins: it is
 * the newest reading the harness reported).
 */
export function compactDialogView({
  cached,
  query,
  changed,
  now = Date.now(),
}: {
  cached: Pick<
    AgentContextUsage,
    "sessionId" | "used" | "size" | "compactSupported" | "updatedAt"
  >;
  query: ContextQueryState;
  changed: ContextReading | null;
  now?: number;
}): CompactDialogView {
  const cachedView = {
    reading: {
      sessionId: cached.sessionId,
      used: cached.used,
      size: cached.size,
      compactSupported: cached.compactSupported,
    },
    updatedAt: cached.updatedAt,
  };
  const liveView = (reading: ContextReading) => ({
    reading: {
      sessionId: reading.sessionId,
      used: reading.used,
      size: reading.size,
      compactSupported: reading.compactSupported,
    },
    updatedAt: reading.updatedAt ?? now,
  });
  const base = { checking: false, noSession: false };

  if (changed) {
    return {
      ...base,
      ...liveView(changed),
      freshness: "changed",
      note: "Context changed since you looked. Re-check the reading above, then compact again.",
    };
  }
  switch (query.status) {
    case "checking":
      return {
        ...cachedView,
        checking: true,
        noSession: false,
        freshness: "checking",
        note: "Checking the session's current context…",
      };
    case "ok":
      return {
        ...base,
        ...liveView(query.reading),
        freshness: "live",
        note: "Confirmed with the agent just now.",
      };
    case "no_session":
      return {
        ...cachedView,
        checking: false,
        noSession: true,
        freshness: "no_session",
        note: "The agent has no live session here. It may have restarted since this reading.",
      };
    case "busy":
      return {
        ...base,
        ...cachedView,
        freshness: "cached",
        note: "The agent is working in this session, so this is the last reading before the current turn.",
      };
    case "no_reading":
      return {
        ...base,
        ...cachedView,
        freshness: "cached",
        note: "The live session hasn't reported its usage yet. Showing the last known reading.",
      };
    case "unconfirmed":
      return {
        ...base,
        ...cachedView,
        freshness: "possibly_stale",
        note: "Couldn't confirm the current context with the agent. This reading may be out of date.",
      };
  }
}

/**
 * Reason the Compact action is unavailable, or null when it can run. The
 * dialog also holds Compact while its live check runs; the view's note
 * already says so.
 */
export function compactUnavailableReason({
  compactSupported,
  hasActiveTurn,
  noSession = false,
  pending,
}: {
  compactSupported: boolean;
  hasActiveTurn: boolean;
  noSession?: boolean;
  pending: boolean;
}): string | null {
  if (noSession) {
    return "There is no live session to compact.";
  }
  if (!compactSupported) {
    return "This agent's runtime doesn't support compaction.";
  }
  if (pending) {
    return "A compaction request for this session is already in progress.";
  }
  if (hasActiveTurn) {
    return "The agent is working in this channel. Compact after the turn finishes.";
  }
  return null;
}
