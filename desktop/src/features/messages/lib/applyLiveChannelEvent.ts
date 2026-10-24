import type { QueryClient } from "@tanstack/react-query";

import type { RelayEvent } from "@/shared/api/types";
import {
  CHANNEL_AUX_EVENT_KINDS,
  CHANNEL_TIMELINE_CONTENT_KINDS,
  KIND_CHANNEL_THREAD_SUMMARY,
} from "@/shared/constants/kinds";
import { forgetChannelSnapshot } from "./channelHeadCache";
import { markChannelNeedsRefresh } from "./channelRoomSyncState";
import { parseLiveThreadSummary } from "./channelWindowResponse";
import {
  boundClosedChannelWindow,
  emptyChannelWindowStore,
  mergeLiveChannelWindowEvent,
  mergeLiveThreadSummary,
  type ChannelWindowStore,
} from "./channelWindowStore";
import { mergeMessages, mergeTimelineCacheMessages } from "./messageMerge";
import {
  channelMessagesKey,
  channelWindowKey,
  threadRepliesKey,
} from "./messageQueryKeys";
import { projectChannelWindowMessages } from "./projectChannelWindow";
import { getThreadReference, isBroadcastReply } from "./threading";

const CHANNEL_TIMELINE_KINDS = new Set<number>(CHANNEL_TIMELINE_CONTENT_KINDS);
const CHANNEL_AUX_KINDS = new Set<number>(CHANNEL_AUX_EVENT_KINDS);

export type ApplyLiveChannelEventOptions = {
  /** The room's timeline is on screen (its live window stays unbounded). */
  isOpen: boolean;
};

/** Whether some surface currently renders this room's timeline. */
export function isChannelTimelineObserved(
  queryClient: QueryClient,
  channelId: string,
): boolean {
  const query = queryClient
    .getQueryCache()
    .find({ queryKey: channelMessagesKey(channelId), exact: true });
  return (query?.getObserversCount() ?? 0) > 0;
}

/**
 * Write one live event into its room's store, whether or not the room is
 * open. The window store is the source of truth; the rendered timeline
 * (`channelMessagesKey`) is re-projected from it only when it already exists
 * (or the room is open), so a room never opened gets no timeline cache — that
 * would read as fresh and paint live rows without their history on open.
 *
 * A room that is not open keeps a bounded live window (see
 * `boundClosedChannelWindow`); once it overflows, its rendered timeline is
 * dropped and it is marked as needing a refresh, so opening it fetches a
 * fresh newest window instead of showing a silent hole.
 */
export function applyLiveChannelEvent(
  queryClient: QueryClient,
  channelId: string,
  event: RelayEvent,
  { isOpen }: ApplyLiveChannelEventOptions,
): void {
  const windowKey = channelWindowKey(channelId);
  const messagesKey = channelMessagesKey(channelId);
  const readWindow = () =>
    queryClient.getQueryData<ChannelWindowStore>(windowKey) ??
    emptyChannelWindowStore();

  if (event.kind === KIND_CHANNEL_THREAD_SUMMARY) {
    // Relay-pushed live badge recount — window-store overlay only, never a
    // timeline row (mirrors the page path, where 39005 is metadata).
    const parsed = parseLiveThreadSummary(event);
    if (!parsed) return;
    const current = readWindow();
    const next = mergeLiveThreadSummary(current, parsed.rootId, parsed.live);
    if (next !== current) queryClient.setQueryData(windowKey, next);
    return;
  }

  const hasTimeline = queryClient.getQueryData(messagesKey) !== undefined;
  const isTimelineRow = CHANNEL_TIMELINE_KINDS.has(event.kind);
  const threadReference = isTimelineRow ? getThreadReference(event.tags) : null;
  if (threadReference?.parentId != null) {
    const rootId = threadReference.rootId;
    if (rootId) {
      const repliesKey = threadRepliesKey(channelId, rootId);
      // A closed room's thread that was never loaded stays unloaded, so its
      // first open still fetches the whole thread.
      if (isOpen || queryClient.getQueryData(repliesKey) !== undefined) {
        queryClient.setQueryData<RelayEvent[]>(repliesKey, (current = []) =>
          mergeMessages(current, event),
        );
      }
    }
    if (!isBroadcastReply(event.tags)) {
      // Reconciliation retains non-broadcast replies in a rendered timeline.
      if (hasTimeline) {
        queryClient.setQueryData<RelayEvent[]>(
          messagesKey,
          (current) => current && mergeTimelineCacheMessages(current, event),
        );
      }
      return;
    }
  }
  if (!isTimelineRow && !CHANNEL_AUX_KINDS.has(event.kind)) return;
  if (!isTimelineRow) {
    queryClient.setQueriesData<RelayEvent[]>(
      { queryKey: ["thread-replies", channelId] },
      (current = []) => mergeMessages(current, event),
    );
  }

  const current = readWindow();
  const merged = mergeLiveChannelWindowEvent(current, event, isTimelineRow);
  if (merged === current) return;
  const next = isOpen ? merged : boundClosedChannelWindow(merged);
  queryClient.setQueryData(windowKey, next);
  if (next !== merged) {
    // The kept rows no longer join the rendered history; showing both would
    // hide the trimmed rows between them. Opening fetches a fresh window.
    markChannelNeedsRefresh(queryClient, channelId);
    forgetChannelSnapshot(queryClient, channelId);
    queryClient.removeQueries({ queryKey: messagesKey, exact: true });
  } else if (isOpen || hasTimeline) {
    projectChannelWindowMessages(queryClient, channelId);
  }
}
