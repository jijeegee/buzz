import type { RelaySubscriptionFilter } from "@/shared/api/relayClientShared";
import { KIND_AGENT_OBSERVER_FRAME } from "@/shared/constants/kinds";

/**
 * Observer frames addressed to the owner, live only. Frames are never stored
 * on the relay, so there is no history to ask for, and a `limit: 0` filter
 * with one `#p` rides the addressed account bundle instead of its own REQ.
 * Older history is served by the archive path (ingestArchivedObserverEvents).
 * Same as mobile (test-fixtures/agent-observer.json).
 */
export function buildObserverLiveFilter(
  ownerPubkey: string,
): RelaySubscriptionFilter {
  return {
    kinds: [KIND_AGENT_OBSERVER_FRAME],
    "#p": [ownerPubkey],
    limit: 0,
  };
}
