import * as React from "react";

import { hasPendingLandAtLatest } from "@/features/channels/lib/landAtLatest";
import {
  type ReadingPosition,
  readingPositionStore,
} from "@/features/channels/readingPositionStore";
import { getThreadReference } from "@/features/messages/lib/threading";
import type { RelayEvent } from "@/shared/api/types";

/**
 * Older pages fetched while looking for the first unread before giving up and
 * opening at the newest message (about 200 top-level rows at 50 per page).
 */
export const MAX_UNREAD_SEEK_PAGES = 4;

type OpenPlan = {
  channelId: string;
  decided: boolean;
  anchorMessageId: string | null;
  /** Where the anchor's row sits when it is a remembered position. */
  anchorTopPx: number | null;
  pagesFetched: number;
  /** Oldest loaded row seen while seeking, to notice lost older pages. */
  seekOldestAt: number | null;
  /** Where the reader left this channel last time, read once per visit. */
  remembered: ReadingPosition | null;
  /**
   * The visit was asked to land at the newest message. Read on the visit's
   * first render, before the landing hook consumes the request.
   */
  landsAtLatest: boolean;
};

type UseChannelOpenAnchorOptions = {
  channelId: string | null;
  /** Account whose remembered reading positions apply. */
  pubkey: string | undefined;
  /** Raw channel events, oldest first; undefined until the first window. */
  messages: RelayEvent[] | undefined;
  /** Read frontier captured when the channel was opened. */
  frontierSeconds: number | null;
  /** First unread top-level row among the loaded ones. */
  firstUnreadMessageId: string | null;
  hasOlderMessages: boolean;
  isFetchingOlder: boolean;
  fetchOlder: () => Promise<void>;
  /** The visit opens somewhere else (a linked message, a huddle transcript). */
  opensElsewhere: boolean;
};

/**
 * Decides once per channel visit where the main timeline opens: at the first
 * unread message when there is one, otherwise where the reader left the
 * channel, otherwise at the newest message. When the row to open at may be
 * older than the loaded ones, older pages are fetched first (up to
 * MAX_UNREAD_SEEK_PAGES) while the timeline keeps its loading state; the list
 * then mounts directly at its final position.
 */
export function useChannelOpenAnchor({
  channelId,
  pubkey,
  messages,
  frontierSeconds,
  firstUnreadMessageId,
  hasOlderMessages,
  isFetchingOlder,
  fetchOlder,
  opensElsewhere,
}: UseChannelOpenAnchorOptions) {
  const planRef = React.useRef<OpenPlan | null>(null);
  if (channelId === null) {
    planRef.current = null;
  } else if (planRef.current?.channelId !== channelId) {
    planRef.current = {
      channelId,
      decided: false,
      anchorMessageId: null,
      anchorTopPx: null,
      pagesFetched: 0,
      seekOldestAt: null,
      remembered: pubkey ? readingPositionStore.get(pubkey, channelId) : null,
      landsAtLatest: hasPendingLandAtLatest(channelId),
    };
  }
  const plan = planRef.current;

  const oldestLoadedAt = messages?.[0]?.created_at ?? null;
  let needsOlder = false;
  // Decide only between page fetches: a page still in flight may hold the
  // row to open at, and counts against the seek budget already.
  if (plan && !plan.decided && messages !== undefined && !isFetchingOlder) {
    // A newest-window refresh (subscribe settle, reconnect) replaces every
    // older page fetched so far; seek again instead of stalling on them.
    if (
      plan.seekOldestAt !== null &&
      oldestLoadedAt !== null &&
      oldestLoadedAt > plan.seekOldestAt
    ) {
      plan.pagesFetched = 0;
    }
    plan.seekOldestAt = oldestLoadedAt;
    const decide = (
      anchorMessageId: string | null,
      anchorTopPx: number | null = null,
    ) => {
      plan.decided = true;
      plan.anchorMessageId = anchorMessageId;
      plan.anchorTopPx = anchorMessageId === null ? null : anchorTopPx;
    };
    const canSeekOlder =
      hasOlderMessages && plan.pagesFetched < MAX_UNREAD_SEEK_PAGES;
    if (opensElsewhere || plan.landsAtLatest) {
      decide(null);
    } else if (frontierSeconds === null || firstUnreadMessageId === null) {
      // Nothing unread: go back to where the reader left, if it is close.
      const remembered = plan.remembered;
      if (remembered === null) {
        decide(null);
      } else if (messages.some((event) => event.id === remembered.messageId)) {
        decide(remembered.messageId, remembered.topPx);
      } else if (canSeekOlder) {
        needsOlder = true;
      } else {
        decide(null);
      }
    } else {
      const oldestTopLevel = messages.find(
        (event) => getThreadReference(event.tags).parentId === null,
      );
      const everyLoadedRowUnread =
        oldestTopLevel !== undefined &&
        oldestTopLevel.created_at > frontierSeconds;
      if (!everyLoadedRowUnread || !hasOlderMessages) {
        decide(firstUnreadMessageId);
      } else if (!canSeekOlder) {
        // Unread for too long: the start is too far back to open at.
        decide(null);
      } else {
        needsOlder = true;
      }
    }
  }

  // A visit that opens anywhere but the remembered row starts the reader
  // somewhere new, so the old spot must not come back on the next visit.
  const staleRemembered =
    plan?.decided === true &&
    plan.remembered !== null &&
    plan.anchorMessageId !== plan.remembered.messageId;
  React.useEffect(() => {
    if (!staleRemembered || !plan || !pubkey) return;
    plan.remembered = null;
    readingPositionStore.set(pubkey, plan.channelId, null);
  }, [plan, pubkey, staleRemembered]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: seek again when the loaded range changes
  React.useEffect(() => {
    if (!needsOlder || isFetchingOlder || !plan) return;
    plan.pagesFetched += 1;
    void fetchOlder();
  }, [fetchOlder, isFetchingOlder, needsOlder, oldestLoadedAt, plan]);

  return {
    anchorMessageId: plan?.decided ? plan.anchorMessageId : null,
    anchorTopPx: plan?.decided ? plan.anchorTopPx : null,
    isResolving: plan !== null && !plan.decided && messages !== undefined,
  };
}
