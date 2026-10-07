import { sendAgentObserverControl } from "@/shared/api/observerRelay";

/** Send a stop request; the harness acknowledges it via control_result. */
export async function cancelManagedAgentTurn(
  pubkey: string,
  channelId: string,
  requestId: string,
): Promise<void> {
  await sendAgentObserverControl(pubkey, {
    type: "cancel_turn",
    channelId,
    requestId,
  });
}

/**
 * Send a live model-switch control frame to a running agent. The switch rides
 * the harness's cancel-switch-requeue path (busy turn) or invalidate-and-reapply
 * (idle); the outcome arrives asynchronously as a `control_result` observer
 * frame, not as the return value here. This is fire-and-forget on the send side.
 *
 * `requestId` is an opaque per-pick correlator the harness echoes back on both
 * the immediate ack and the late terminal frame, so a reconnect replay of an
 * earlier pick's result cannot settle this one.
 */
export async function switchManagedAgentModel(
  pubkey: string,
  channelId: string,
  modelId: string,
  requestId: string,
): Promise<void> {
  await sendAgentObserverControl(pubkey, {
    type: "switch_model",
    channelId,
    modelId,
    requestId,
  });
}

/** The reading the user confirmed before asking for compaction. */
export type CompactSessionExpectation = {
  sessionId: string | null;
  used: number;
};

/**
 * Ask a running agent to compact one session's context. `threadRootEventId`
 * selects the thread-scoped session; null targets the channel's whole
 * conversation scope. The harness acknowledges with a `started` control
 * result and later reports a terminal status for the same `requestId`.
 *
 * `expected` carries the reading the user saw; the harness refuses with
 * `stale` (and its current reading) when the session changed since then.
 * Harnesses without the guard ignore the extra fields.
 */
export async function compactManagedAgentSession(
  pubkey: string,
  channelId: string,
  threadRootEventId: string | null,
  requestId: string,
  expected?: CompactSessionExpectation,
): Promise<void> {
  await sendAgentObserverControl(pubkey, {
    type: "compact_session",
    channelId,
    threadRootEventId,
    requestId,
    ...(expected?.sessionId ? { expectedSessionId: expected.sessionId } : {}),
    ...(expected ? { expectedUsed: expected.used } : {}),
  });
}

/**
 * Ask a running agent for one session scope's latest context reading. The
 * harness answers with a `query_context_usage` control result for the same
 * `requestId`: `ok` with a reading, `no_reading`, `busy`, or `no_session`.
 */
export async function queryManagedAgentContextUsage(
  pubkey: string,
  channelId: string,
  threadRootEventId: string | null,
  requestId: string,
): Promise<void> {
  await sendAgentObserverControl(pubkey, {
    type: "query_context_usage",
    channelId,
    threadRootEventId,
    requestId,
  });
}
