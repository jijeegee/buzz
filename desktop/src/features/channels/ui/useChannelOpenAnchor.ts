import * as React from "react";

import { useAppShell } from "@/app/AppShellContext";
import { hasPendingLandAtLatest } from "@/features/channels/lib/landAtLatest";
import {
  type ReadingPosition,
  readingPositionStore,
} from "@/features/channels/readingPositionStore";
import {
  decideOpenAnchor,
  reanchorTarget,
  roomPaintDecides,
} from "@/features/channels/lib/openAnchorDecision";
import { paintCoverage } from "@/features/messages/lib/paintCoverage";
import { getThreadReference } from "@/features/messages/lib/threading";
import type { RelayEvent } from "@/shared/api/types";

import { useReaderMoved } from "./useReaderMoved";

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

export type OpenWindowCommit = {
  at: number;
  newestRowAt: number | null;
  /** The window's newest row; the visit decides once its rows show it. */
  newestRowId?: string | null;
};

/**
 * Whether the room's window is current enough to decide for good where the
 * visit opens: a window fetched during this visit (from the relay, or a
 * certain one from the device store), or one fetched earlier whose newest row
 * is at least as new as the channel's last message. A stored paint, a head
 * cache from an earlier run, or rows that arrived live on an older window can
 * miss newer unread rows: a decision on them is provisional.
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
  /**
   * Decided on rows that are not a current window: once one is shown the
   * rule runs again, and the open moves if the reader has not scrolled.
   */
  provisional: boolean;
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
 * channel, otherwise at the newest message. It decides on the first rows held,
 * stored or fetched (decideOpenAnchor); on rows that are not a current window
 * (isOpenWindowCurrent) the decision is provisional and runs again once a
 * current window is shown, moving the open only if the reader has not
 * scrolled. When the row to open at may be older than the loaded ones, older
 * pages are fetched first (up to MAX_UNREAD_SEEK_PAGES) while the timeline
 * keeps its loading state; the list then mounts directly at its position.
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
  // An open on the device's read markers stays provisional until the
  // relay's have been taken in (useChannelUnreadState retakes them).
  const { isReadStateSettled } = useAppShell();
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
      provisional: false,
      keepsRemembered: false,
      keptOnOpen: false,
    };
  }
  const plan = planRef.current;

  let needsOlder = false;
  const readerMoved = React.useRef(false);
  if (plan && messages !== undefined && (!plan.decided || plan.provisional)) {
    const current =
      isOpenWindowCurrent(
        windowCommit,
        plan.visitStartedAt,
        lastMessageAtSeconds,
      ) && windowRowsShown(windowCommit, messages);
    const oldestTopLevel = messages.find(
      (event) => getThreadReference(event.tags).parentId === null,
    );
    const decision = decideOpenAnchor({
      opensElsewhere: opensElsewhere || plan.landsAtLatest,
      frontierSeconds,
      frontierUnknown,
      firstUnreadMessageId,
      remembered: plan.remembered,
      hasRow: (id) => messages.some((event) => event.id === id),
      oldestTopLevelAt: oldestTopLevel?.created_at ?? null,
      hasOlderMessages,
      canSeekOlder:
        hasOlderMessages &&
        !plan.seekExpired &&
        plan.pagesFetched < MAX_UNREAD_SEEK_PAGES,
      seekExpired: plan.seekExpired,
    });
    if (plan.decided) {
      // A current window is shown: a provisional open follows it unless the
      // reader has moved, seeking older pages first when it must.
      if (current) {
        if (isReadStateSettled) plan.provisional = false;
        const next = reanchorTarget(
          plan.anchorMessageId,
          decision,
          readerMoved.current,
        );
        if (next?.kind === "open") {
          plan.anchorMessageId = next.anchorMessageId;
          plan.anchorTopPx = next.anchorTopPx;
          plan.keptOnOpen = false;
          plan.keepsRemembered = false;
        } else if (next?.kind === "seek") {
          plan.decided = false;
          plan.provisional = false;
          needsOlder = true;
        }
      }
    } else if (frontierPending && !(opensElsewhere || plan.landsAtLatest)) {
      // Unread rows and the remembered row are only known with the frontier.
    } else if (
      !current &&
      !roomPaintDecides(
        paintCoverage(channelId),
        opensElsewhere || plan.landsAtLatest ? null : firstUnreadMessageId,
        frontierSeconds,
      )
    ) {
      // Stored rows with a hole after the read marker: an earlier unread row
      // may be missing, so the relay's window decides.
    } else if (decision.kind === "seek") {
      needsOlder = true;
    } else {
      plan.decided = true;
      plan.provisional = !current || !isReadStateSettled;
      plan.anchorMessageId = decision.anchorMessageId;
      plan.anchorTopPx = decision.anchorTopPx;
      plan.keptOnOpen = decision.keepsRemembered && plan.remembered !== null;
      plan.keepsRemembered = plan.keptOnOpen;
    }
  }

  // A visit that opens anywhere but the remembered row starts the reader
  // somewhere new, so the old spot must not come back on the next visit.
  // A visit that ran out of time (a slow relay) or opened without the read
  // state keeps the spot for the next one, and a provisional open keeps it
  // until the relay's window settles where the visit stays.
  useReaderMoved(
    readerMoved,
    plan?.decided === true && plan.provisional,
    channelId,
  );
  const staleRemembered =
    plan?.decided === true &&
    !plan.provisional &&
    plan.remembered !== null &&
    plan.anchorMessageId !== plan.remembered.messageId &&
    !plan.keptOnOpen;
  React.useEffect(() => {
    if (!staleRemembered || !plan?.scope) return;
    plan.remembered = null;
    readingPositionStore.set(plan.scope, plan.channelId, null);
  }, [plan, staleRemembered]);

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
