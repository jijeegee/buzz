import * as React from "react";

import { hasPendingLandAtLatest } from "@/features/channels/lib/landAtLatest";
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
  pagesFetched: number;
  /**
   * The visit was asked to land at the newest message. Read on the visit's
   * first render, before the landing hook consumes the request.
   */
  landsAtLatest: boolean;
};

type UseChannelOpenAnchorOptions = {
  channelId: string | null;
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
 * unread message when there is one, otherwise at the newest message. When
 * every loaded row is unread the first unread may be older, so older pages
 * are fetched first (up to MAX_UNREAD_SEEK_PAGES) while the timeline keeps
 * its loading state; the list then mounts directly at its final position.
 */
export function useChannelOpenAnchor({
  channelId,
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
      pagesFetched: 0,
      landsAtLatest: hasPendingLandAtLatest(channelId),
    };
  }
  const plan = planRef.current;

  let needsOlder = false;
  if (plan && !plan.decided && messages !== undefined) {
    const decide = (anchorMessageId: string | null) => {
      plan.decided = true;
      plan.anchorMessageId = anchorMessageId;
    };
    if (
      opensElsewhere ||
      frontierSeconds === null ||
      firstUnreadMessageId === null ||
      plan.landsAtLatest
    ) {
      decide(null);
    } else {
      const oldestTopLevel = messages.find(
        (event) => getThreadReference(event.tags).parentId === null,
      );
      const everyLoadedRowUnread =
        oldestTopLevel !== undefined &&
        oldestTopLevel.created_at > frontierSeconds;
      if (!everyLoadedRowUnread || !hasOlderMessages) {
        decide(firstUnreadMessageId);
      } else if (plan.pagesFetched >= MAX_UNREAD_SEEK_PAGES) {
        // Unread for too long: the start is too far back to open at.
        decide(null);
      } else {
        needsOlder = true;
      }
    }
  }

  React.useEffect(() => {
    if (!needsOlder || isFetchingOlder || !plan) return;
    plan.pagesFetched += 1;
    void fetchOlder();
  }, [fetchOlder, isFetchingOlder, needsOlder, plan]);

  return {
    anchorMessageId: plan?.decided ? plan.anchorMessageId : null,
    isResolving: plan !== null && !plan.decided && messages !== undefined,
  };
}
