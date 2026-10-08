import { useQuery } from "@tanstack/react-query";

import {
  getDeletedEventIds,
  INBOX_DELETIONS_QUERY_KEY,
} from "@/features/home/lib/inboxDeletions";
import { relayClient } from "@/shared/api/relayClient";
import { buildChannelAuxDeletionFilter } from "@/shared/api/relayChannelFilters";
import { getHomeFeed } from "@/shared/api/tauri";
import { useRelayConnection } from "@/shared/api/useRelayConnection";
import { useFocusedRefetchInterval } from "@/shared/lib/useDocumentVisible";

/** Keeps focused polling at the established 30-second cadence. */
export const HOME_FEED_REFETCH_INTERVAL_MS = 30_000;
/** Suppresses the expensive focus refetch until the home feed is old. */
export const HOME_FEED_FOCUS_STALE_TIME_MS = 5 * 60_000;

/** Focus-refetch policy for the home feed query; consumed by focusRefetchPolicy.test.mjs. */
export const homeFeedFocusRefetchPolicy = {
  staleTime: HOME_FEED_FOCUS_STALE_TIME_MS,
  refetchOnWindowFocus: false,
} as const;

export function useHomeFeedQuery() {
  const connectionState = useRelayConnection();
  const connected = connectionState === "connected";
  const refetchInterval = useFocusedRefetchInterval(
    connected ? HOME_FEED_REFETCH_INTERVAL_MS : false,
  );

  return useQuery({
    queryKey: ["home-feed"],
    queryFn: () =>
      getHomeFeed({
        limit: 50,
        types: "mentions,needs_action,activity,agent_activity",
      }),
    gcTime: 5 * 60 * 1_000,
    // Pause background polling on degraded/stalled/disconnected connections.
    // The relay can't serve the request anyway, and the spurious failures
    // consume quota that the recovery path needs.
    refetchInterval,
    ...homeFeedFocusRefetchPolicy,
  });
}

/**
 * Deletions of the messages (and their thread roots/parents) behind inbox
 * rows, so a deleted message or thread drops out of the inbox. Asks the relay
 * for deletion events by reference rather than treating a missing event as
 * deleted, so a slow or failed lookup never hides a live row.
 */
export function useInboxDeletedEventIds(referencedEventIds: readonly string[]) {
  const connectionState = useRelayConnection();
  const connected = connectionState === "connected";
  const refetchInterval = useFocusedRefetchInterval(
    connected ? HOME_FEED_REFETCH_INTERVAL_MS : false,
  );
  const idsKey = referencedEventIds.join(",");

  return useQuery({
    enabled: connected && referencedEventIds.length > 0,
    queryKey: [...INBOX_DELETIONS_QUERY_KEY, idsKey],
    queryFn: async () =>
      getDeletedEventIds(
        await relayClient.fetchAuxEventsByReference(
          "",
          [...referencedEventIds],
          buildChannelAuxDeletionFilter,
        ),
      ),
    placeholderData: (previous) => previous,
    refetchInterval,
    staleTime: HOME_FEED_REFETCH_INTERVAL_MS,
  });
}
