import type { RelaySubscriptionFilter } from "@/shared/api/relayClientShared";
import { KIND_AGENT_OBSERVER_FRAME } from "@/shared/constants/kinds";

/**
 * How far back (in seconds) the live subscription looks on connect/reconnect.
 * session/prompt is the first frame emitted at turn start, so it can arrive
 * before the subscription when the agent was already running. A 5-minute
 * lookback covers long-running active turns (coding/review turns routinely
 * exceed 60s). Same as mobile (test-fixtures/agent-observer.json).
 */
export const OBSERVER_LIVE_LOOKBACK_SECS = 300;
/** Lets reconnect replay recover frames missed during a drop. */
export const OBSERVER_LIVE_LIMIT = 1000;

/**
 * Observer frames addressed to the owner. Older history is served by the
 * archive path (ingestArchivedObserverEvents); the appendAgentEvent dedup on
 * (seq, timestamp) prevents double-processing of the overlap.
 */
export function buildObserverLiveFilter(
  ownerPubkey: string,
  nowSecs: number,
): RelaySubscriptionFilter {
  return {
    kinds: [KIND_AGENT_OBSERVER_FRAME],
    "#p": [ownerPubkey],
    limit: OBSERVER_LIVE_LIMIT,
    since: nowSecs - OBSERVER_LIVE_LOOKBACK_SECS,
  };
}
