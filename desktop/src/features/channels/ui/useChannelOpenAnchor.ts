import * as React from "react";

import { hasPendingLandAtLatest } from "@/features/channels/lib/landAtLatest";
import {
  type ReadingPosition,
  readingPositionStore,
} from "@/features/channels/readingPositionStore";
import { getThreadReference } from "@/features/messages/lib/threading";
import type { RelayEvent } from "@/shared/api/types";

/**
 * Older pages fetched while looking for the first unread or the remembered
 * row before giving up and opening at the newest message: three pages of the
 * relay's largest window (200 rows) behind the newest 50, about 650 rows.
 */
export const MAX_UNREAD_SEEK_PAGES = 3;
export const UNREAD_SEEK_PAGE_SIZE = 200;
/** A seek that has not found its row by then opens at the newest message. */
export const UNREAD_SEEK_DEADLINE_MS = 2000;
/** Pause before a failed seek page is asked again. */
export const UNREAD_SEEK_RETRY_MS = 250;

/**
 * How long a visit waits for a current window before deciding on the rows it
 * has (an offline relay must not hold the skeleton forever).
 */
export const OPEN_WINDOW_WAIT_MS = 1500;

export type OpenWindowCommit = {
  at: number;
  newestRowAt: number | null;
  /** The window's newest row; the visit decides once its rows show it. */
  newestRowId?: string | null;
};

/**
 * Whether the room's window is current enough to decide where the visit
 * opens: a window fetched during this visit (from the relay, or a certain one
 * from the device store), or one fetched earlier whose newest row is at least
 * as new as the channel's last message. A head cache from an earlier run, or
 * rows that arrived live on an older window, can miss newer unread rows, and
 * the relay page that replaces them may not join them.
 */
export function isOpenWindowCurrent(
  commit: OpenWindowCommit | null,
  visitStartedAt: number,
  lastMessageAtSeconds: number | null,
) {
  if (commit === null) return false;
  if (commit.at >= visitStartedAt) return true;
  return (
    lastMessageAtSeconds !== null &&
    commit.newestRowAt !== null &&
    commit.newestRowAt >= lastMessageAtSeconds
  );
}

/**
 * Whether the rendered rows already hold the committed window's newest row:
 * the commit is announced before the rows it committed reach the timeline.
 */
function windowRowsShown(
  commit: OpenWindowCommit | null,
  messages: readonly { id: string }[],
) {
  const newestRowId = commit?.newestRowId;
  return !newestRowId || messages.some((event) => event.id === newestRowId);
}

type OpenPlan = {
  channelId: string;
  decided: boolean;
  anchorMessageId: string | null;
  /** Where the anchor's row sits when it is a remembered position. */
  anchorTopPx: number | null;
  pagesFetched: number;
  /** When the first older page was asked for; null before any. */
  seekStartedAt: number | null;
  /** UNREAD_SEEK_DEADLINE_MS passed before the row was found. */
  seekExpired: boolean;
  /** A seek page is out, or a failed one waits UNREAD_SEEK_RETRY_MS. */
  seekBusy: boolean;
  /** Where the reader left this channel last time, read once per visit. */
  /** The record this visit reads and writes, fixed for the visit. */
  scope: string | null;
  /**
   * The visit fell back to the newest with a remembered row: being at the
   * newest does not forget it until the reader has scrolled away once.
   */
  keepsRemembered: boolean;
  /** keepsRemembered as decided, before any report. */
  keptOnOpen: boolean;
  remembered: ReadingPosition | null;
  /**
   * The visit was asked to land at the newest message. Read on the visit's
   * first render, before the landing hook consumes the request.
   */
  landsAtLatest: boolean;
  visitStartedAt: number;
  /** OPEN_WINDOW_WAIT_MS passed without a current window. */
  waitExpired: boolean;
};

type UseChannelOpenAnchorOptions = {
  channelId: string | null;
  /** readingPositionScope of the reader; null keeps no position. */
  positionScope: string | null;
  /**
   * The frontier was taken without the read state: no remembered row is
   * trusted, as on mobile.
   */
  frontierUnknown?: boolean;
  /** Raw channel events, oldest first; undefined until the first window. */
  messages: RelayEvent[] | undefined;
  /** The read frontier is not known yet (the read state is loading). */
  frontierPending?: boolean;
  /** The room's last fetched window (channelWindowCommit), if any. */
  windowCommit: OpenWindowCommit | null;
  /** The channel's last message time from the channel list, if known. */
  lastMessageAt: string | null;
  /** Read frontier captured when the channel was opened. */
  frontierSeconds: number | null;
  /** First unread top-level row among the loaded ones. */
  firstUnreadMessageId: string | null;
  hasOlderMessages: boolean;
  isFetchingOlder: boolean;
  /** Fetches one older page; resolves whether it was appended. */
  fetchOlder: (pageSize?: number) => Promise<boolean>;
  /** The visit opens somewhere else (a linked message, a huddle transcript). */
  opensElsewhere: boolean;
};

/**
 * Decides once per channel visit where the main timeline opens: at the first
 * unread message when there is one, otherwise where the reader left the
 * channel, otherwise at the newest message. It decides only on a current
 * window (isOpenWindowCurrent), unless nothing can be unread and there is no
 * remembered row. When the row to open at may be older than the loaded ones,
 * older pages are fetched first (up to MAX_UNREAD_SEEK_PAGES) while the
 * timeline keeps its loading state; the list then mounts directly at its
 * final position.
 */
export function useChannelOpenAnchor({
  channelId,
  positionScope,
  messages,
  windowCommit,
  frontierPending = false,
  frontierUnknown = false,
  lastMessageAt,
  frontierSeconds,
  firstUnreadMessageId,
  hasOlderMessages,
  isFetchingOlder,
  fetchOlder,
  opensElsewhere,
}: UseChannelOpenAnchorOptions) {
  const [tick, rerender] = React.useReducer((count: number) => count + 1, 0);
  const parsedLastMessageAt = lastMessageAt ? Date.parse(lastMessageAt) : NaN;
  const lastMessageAtSeconds = Number.isNaN(parsedLastMessageAt)
    ? null
    : Math.floor(parsedLastMessageAt / 1000);
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
      seekStartedAt: null,
      seekExpired: false,
      seekBusy: false,
      scope: positionScope,
      remembered: positionScope
        ? readingPositionStore.get(positionScope, channelId)
        : null,
      landsAtLatest: hasPendingLandAtLatest(channelId),
      visitStartedAt: Date.now(),
      waitExpired: false,
      keepsRemembered: false,
      keptOnOpen: false,
    };
  }
  const plan = planRef.current;

  let needsOlder = false;
  let waitsForWindow = false;
  if (plan && !plan.decided && messages !== undefined) {
    const decide = (
      anchorMessageId: string | null,
      anchorTopPx: number | null = null,
      keepsRemembered = false,
    ) => {
      plan.decided = true;
      plan.anchorMessageId = anchorMessageId;
      plan.anchorTopPx = anchorMessageId === null ? null : anchorTopPx;
      plan.keptOnOpen = keepsRemembered && plan.remembered !== null;
      plan.keepsRemembered = plan.keptOnOpen;
    };
    const canSeekOlder =
      hasOlderMessages &&
      !plan.seekExpired &&
      plan.pagesFetched < MAX_UNREAD_SEEK_PAGES;
    const nothingUnread =
      frontierSeconds !== null &&
      lastMessageAtSeconds !== null &&
      lastMessageAtSeconds <= frontierSeconds;
    if (opensElsewhere || plan.landsAtLatest) {
      decide(null);
    } else if (frontierPending) {
      // Unread rows and the remembered row are only known with the frontier.
    } else if (
      !plan.waitExpired &&
      !(
        isOpenWindowCurrent(
          windowCommit,
          plan.visitStartedAt,
          lastMessageAtSeconds,
        ) && windowRowsShown(windowCommit, messages)
      )
    ) {
      if (nothingUnread && plan.remembered === null) decide(null);
      else waitsForWindow = true;
    } else if (frontierSeconds === null || firstUnreadMessageId === null) {
      // Nothing unread: go back to where the reader left, if it is close.
      const remembered = frontierUnknown ? null : plan.remembered;
      if (remembered === null) {
        decide(null, null, frontierUnknown);
      } else if (messages.some((event) => event.id === remembered.messageId)) {
        decide(remembered.messageId, remembered.topPx);
      } else if (canSeekOlder) {
        needsOlder = true;
      } else {
        // Out of time (a slow relay) keeps the row; out of pages forgets it.
        decide(null, null, plan.seekExpired || plan.waitExpired);
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
  // A visit that ran out of time (a slow relay) or opened without the read
  // state keeps the spot for the next one.
  const staleRemembered =
    plan?.decided === true &&
    plan.remembered !== null &&
    plan.anchorMessageId !== plan.remembered.messageId &&
    !plan.keptOnOpen;
  React.useEffect(() => {
    if (!staleRemembered || !plan?.scope) return;
    plan.remembered = null;
    readingPositionStore.set(plan.scope, plan.channelId, null);
  }, [plan, staleRemembered]);

  React.useEffect(() => {
    if (!waitsForWindow || !plan) return;
    const timer = window.setTimeout(
      () => {
        plan.waitExpired = true;
        rerender();
      },
      Math.max(0, plan.visitStartedAt + OPEN_WINDOW_WAIT_MS - Date.now()),
    );
    return () => window.clearTimeout(timer);
  }, [plan, waitsForWindow]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: tick re-checks a settled page
  React.useEffect(() => {
    if (!needsOlder || isFetchingOlder || !plan || plan.seekBusy) return;
    plan.seekStartedAt ??= Date.now();
    plan.pagesFetched += 1;
    plan.seekBusy = true;
    // A failed page (an error, or a chain the relay's newest page just
    // re-cut) is asked again after a pause rather than counted; the deadline
    // bounds it.
    void fetchOlder(UNREAD_SEEK_PAGE_SIZE).then((appended) => {
      if (appended) {
        plan.seekBusy = false;
        rerender();
        return;
      }
      window.setTimeout(() => {
        plan.pagesFetched -= 1;
        plan.seekBusy = false;
        rerender();
      }, UNREAD_SEEK_RETRY_MS);
    });
  }, [fetchOlder, isFetchingOlder, needsOlder, plan, tick]);

  const seeking = needsOlder && plan?.seekStartedAt != null;
  React.useEffect(() => {
    if (!seeking || !plan || plan.seekStartedAt === null) return;
    const timer = window.setTimeout(
      () => {
        plan.seekExpired = true;
        rerender();
      },
      Math.max(0, plan.seekStartedAt + UNREAD_SEEK_DEADLINE_MS - Date.now()),
    );
    return () => window.clearTimeout(timer);
  }, [plan, seeking]);

  /** Remembers where the reader is in this room; null forgets it. */
  const savePosition = React.useCallback(
    (position: ReadingPosition | null) => {
      if (!plan?.scope) return;
      if (position === null && plan.keepsRemembered) return;
      plan.keepsRemembered = false;
      readingPositionStore.set(plan.scope, plan.channelId, position);
    },
    [plan],
  );

  return {
    savePosition,
    anchorMessageId: plan?.decided ? plan.anchorMessageId : null,
    anchorTopPx: plan?.decided ? plan.anchorTopPx : null,
    isResolving: plan !== null && !plan.decided && messages !== undefined,
  };
}
