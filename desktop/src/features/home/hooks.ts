import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import * as React from "react";

import {
  getDeletedEventIds,
  INBOX_DELETIONS_QUERY_KEY,
} from "@/features/home/lib/inboxDeletions";
import {
  INBOX_SUMMARY_BACKGROUND_ROWS,
  INBOX_SUMMARY_PAGE_SIZE,
  INBOX_SUMMARY_QUERY_KEY,
  inboxSummaryFeedItems,
  inboxSummaryUnread,
  mergeInboxSummaryPage,
  type InboxSummaryState,
} from "@/features/home/lib/inboxSummary";
import { getInboxSummaryPage } from "@/shared/api/inboxSummary";
import { relayClient } from "@/shared/api/relayClient";
import { buildChannelAuxDeletionFilter } from "@/shared/api/relayChannelFilters";
import { getHomeFeed } from "@/shared/api/tauri";
import type { Channel, FeedItem } from "@/shared/api/types";
import {
  KIND_STREAM_MESSAGE,
  KIND_STREAM_MESSAGE_V2,
} from "@/shared/constants/kinds";
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

/** Most recently active joined rooms the Channels + Threads view loads. */
const CONVERSATION_CHANNEL_LIMIT = 50;
/** Recent events per room: enough for its latest message and live threads. */
const CONVERSATION_EVENTS_PER_CHANNEL = 30;

// Module-level so useQueries can keep the combined array stable between
// renders when no room's data changed.
function combineRoomActivity(
  results: readonly { data?: FeedItem[] | undefined }[],
): FeedItem[] {
  return results.flatMap((result) => result.data ?? []);
}

async function fetchRoomActivity(channel: Channel): Promise<FeedItem[]> {
  const events = await relayClient.fetchEvents({
    kinds: [KIND_STREAM_MESSAGE, KIND_STREAM_MESSAGE_V2],
    "#h": [channel.id],
    limit: CONVERSATION_EVENTS_PER_CHANNEL,
  });
  return events.map((event) => ({
    category: "activity",
    channelId: channel.id,
    channelName: channel.name,
    channelType: channel.channelType,
    content: event.content,
    createdAt: event.created_at,
    id: event.id,
    kind: event.kind,
    pubkey: event.pubkey,
    tags: event.tags,
  }));
}

/**
 * Recent traffic of the user's rooms for the Channels + Threads view, from
 * everyone including the user: that view is a chat list, so your own last
 * message counts as the room's activity. One cached query per room, keyed by
 * its last-message time (kept current by live updates), so a new message
 * refetches only its own room and quiet rooms are never crowded out.
 */
export function useConversationChannelActivity(
  channels: readonly Channel[] | undefined,
  enabled: boolean,
): FeedItem[] {
  const connectionState = useRelayConnection();
  const active = enabled && connectionState === "connected";
  const rooms = React.useMemo(
    () =>
      (channels ?? [])
        .filter((channel) => channel.isMember && channel.archivedAt === null)
        .sort((a, b) =>
          (b.lastMessageAt ?? "").localeCompare(a.lastMessageAt ?? ""),
        )
        .slice(0, CONVERSATION_CHANNEL_LIMIT),
    [channels],
  );

  return useQueries({
    queries: rooms.map((channel) => ({
      enabled: active,
      queryFn: () => fetchRoomActivity(channel),
      queryKey: [
        "conversation-room-activity",
        channel.id,
        channel.lastMessageAt ?? "",
      ],
      placeholderData: (previous: FeedItem[] | undefined) => previous,
      staleTime: Number.POSITIVE_INFINITY,
    })),
    combine: combineRoomActivity,
  });
}

let inboxSummaryGeneration = 0;

/**
 * The Channels + Threads view from one relay summary: member rooms and their
 * replied-to threads, newest first, with server unread counts. The first page
 * renders immediately; later pages load one after another in the background
 * up to INBOX_SUMMARY_BACKGROUND_ROWS. Live events patch rows in place
 * (useLiveChannelUpdates), so a new message never refetches anything.
 * `failed` lets callers fall back to per-room loading on relays without the
 * summary envelope.
 */
export function useInboxSummary(
  channels: readonly Channel[] | undefined,
  enabled: boolean,
  getReadContexts: () => Record<string, number>,
  readStateVersion: number,
) {
  const connectionState = useRelayConnection();
  const active = enabled && connectionState === "connected";
  const queryClient = useQueryClient();
  const query = useQuery({
    enabled: active,
    queryKey: INBOX_SUMMARY_QUERY_KEY,
    queryFn: async (): Promise<InboxSummaryState> => {
      const read = getReadContexts();
      return {
        ...(await getInboxSummaryPage(null, read, INBOX_SUMMARY_PAGE_SIZE)),
        generation: ++inboxSummaryGeneration,
        sentReadMarkers: Object.keys(read).length > 0,
        sentRead: read,
      };
    },
    refetchOnWindowFocus: false,
    retry: 1,
    staleTime: Number.POSITIVE_INFINITY,
  });
  const data = query.data;
  // Read state loads after startup; count unread once it is available.
  const needsReadMarkers = data !== undefined && !data.sentReadMarkers;
  // biome-ignore lint/correctness/useExhaustiveDependencies: readStateVersion signals that read markers changed
  React.useEffect(() => {
    if (
      needsReadMarkers &&
      Object.keys(getReadContexts()).length > 0 &&
      !query.isFetching
    ) {
      void queryClient.invalidateQueries({ queryKey: INBOX_SUMMARY_QUERY_KEY });
    }
  }, [needsReadMarkers, readStateVersion, query.isFetching]);
  const next = data?.next ?? null;
  const wantsMore =
    active &&
    next !== null &&
    (data?.rows.length ?? 0) < INBOX_SUMMARY_BACKGROUND_ROWS;
  const generation = data?.generation;
  const sentRead = data?.sentRead;
  // biome-ignore lint/correctness/useExhaustiveDependencies: keyed by the cursor; live row patches must not restart a page in flight
  React.useEffect(() => {
    if (!wantsMore || next === null) return;
    let cancelled = false;
    // Later pages count against the same markers as the first page.
    getInboxSummaryPage(next, sentRead ?? {}, INBOX_SUMMARY_PAGE_SIZE)
      .then((page) => {
        if (cancelled) return;
        queryClient.setQueryData<InboxSummaryState>(
          INBOX_SUMMARY_QUERY_KEY,
          (current) =>
            // A newer first page or another page landed meanwhile.
            current !== undefined &&
            current.generation === generation &&
            current.next?.key === next.key &&
            current.next.at_us === next.at_us
              ? mergeInboxSummaryPage(current, page)
              : current,
        );
      })
      .catch((error: unknown) => {
        // The next summary refetch (reconnect, deletion) restarts paging.
        console.warn("inbox summary page failed", error);
      });
    return () => {
      cancelled = true;
    };
  }, [generation, next?.at_us, next?.key, wantsMore]);

  const channelById = React.useMemo(
    () => new Map((channels ?? []).map((channel) => [channel.id, channel])),
    [channels],
  );
  const rows = data?.rows;
  const feedItems = React.useMemo(
    () => (rows ? inboxSummaryFeedItems(rows, channelById) : []),
    [channelById, rows],
  );
  const unread = React.useMemo(
    () => inboxSummaryUnread(rows ?? [], sentRead ?? {}),
    [rows, sentRead],
  );
  return { failed: query.isError && !data, feedItems, unread };
}
