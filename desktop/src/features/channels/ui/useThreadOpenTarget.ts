import * as React from "react";

import {
  type ReadingPosition,
  readingPositionStore,
} from "@/features/channels/readingPositionStore";
import type { MainTimelineEntry } from "@/features/messages/lib/threadPanel";

import {
  MAX_UNREAD_SEEK_PAGES,
  UNREAD_SEEK_DEADLINE_MS,
  UNREAD_SEEK_PAGE_SIZE,
  UNREAD_SEEK_RETRY_MS,
} from "./useChannelOpenAnchor";
import type { ThreadOpenRead } from "./useChannelUnreadState";

/** A first unread reply sits this far below the top of the thread view. */
const UNREAD_REPLY_TOP_PX = 8;

/** The reading-position key of a thread, as on mobile. */
export function threadPositionKey(channelId: string, rootId: string) {
  return `${channelId}/${rootId}`;
}

type ThreadOpenPlan = {
  key: string;
  decided: boolean;
  targetId: string | null;
  topPx: number | null;
  released: boolean;
  /** The record this visit reads and writes, fixed for the visit. */
  scope: string | null;
  remembered: ReadingPosition | null;
  pagesFetched: number;
  seekStartedAt: number | null;
  seekExpired: boolean;
  /** A seek page is out, or a failed one waits UNREAD_SEEK_RETRY_MS. */
  seekBusy: boolean;
  /** As in rooms: a fallback to the newest keeps the remembered reply. */
  keepsRemembered: boolean;
  /** keepsRemembered as decided, before any report. */
  keptOnOpen: boolean;
};

type UseThreadOpenTargetOptions = {
  channelId: string | null;
  rootId: string | null;
  /** readingPositionScope of the reader; null keeps no position. */
  positionScope: string | null;
  /** A link or a view switch already says where the thread opens. */
  opensElsewhere: boolean;
  replies: readonly MainTimelineEntry[];
  isLoading: boolean;
  firstUnreadReplyId: string | null;
  /** Nothing is decided while "pending"; "missed" trusts no position. */
  readState?: ThreadOpenRead;
  hasOlderReplies: boolean;
  isFetchingOlder: boolean;
  /** Resolves whether an older page was added. */
  fetchOlder: ((limitRows?: number) => Promise<boolean>) | undefined;
};

/**
 * Where a thread opens, decided once per visit as rooms do: a linked reply
 * (the caller's own target), else the first unread reply, else where the
 * reader left it, else the newest reply. A reply in an older page is fetched
 * first, with a room's limits; until then the thread reports `isResolving` so
 * nothing is drawn elsewhere first (docs/message-sync.md#opening-at-a-position).
 */
export function useThreadOpenTarget({
  channelId,
  rootId,
  positionScope,
  opensElsewhere,
  replies,
  isLoading,
  firstUnreadReplyId,
  readState = "ready",
  hasOlderReplies,
  isFetchingOlder,
  fetchOlder,
}: UseThreadOpenTargetOptions) {
  const [tick, rerender] = React.useReducer((count: number) => count + 1, 0);
  const key = channelId && rootId ? threadPositionKey(channelId, rootId) : null;
  const planRef = React.useRef<ThreadOpenPlan | null>(null);
  if (key === null) {
    planRef.current = null;
  } else if (planRef.current?.key !== key) {
    planRef.current = {
      key,
      decided: false,
      targetId: null,
      topPx: null,
      released: false,
      scope: positionScope,
      remembered: positionScope
        ? readingPositionStore.get(positionScope, key)
        : null,
      pagesFetched: 0,
      seekStartedAt: null,
      seekExpired: false,
      seekBusy: false,
      keepsRemembered: false,
      keptOnOpen: false,
    };
  }
  const plan = planRef.current;

  let needsOlder = false;
  const waitsForReadState = readState === "pending" && !opensElsewhere;
  if (plan && !plan.decided && !isLoading && !waitsForReadState) {
    const decide = (
      targetId: string | null,
      topPx: number | null = null,
      keepsRemembered = false,
    ) => {
      plan.decided = true;
      plan.targetId = targetId;
      plan.topPx = targetId === null ? null : topPx;
      plan.keptOnOpen = keepsRemembered && plan.remembered !== null;
      plan.keepsRemembered = plan.keptOnOpen;
    };
    const canSeekOlder =
      hasOlderReplies &&
      fetchOlder !== undefined &&
      !plan.seekExpired &&
      plan.pagesFetched < MAX_UNREAD_SEEK_PAGES;
    const has = (id: string) =>
      replies.some((reply) => reply.message.id === id);
    // Without the read state every reply reads as unread: trust neither.
    const missed = readState === "missed";
    const remembered = missed ? null : plan.remembered;
    const firstUnread = missed ? null : firstUnreadReplyId;
    if (opensElsewhere) {
      decide(null);
    } else if (firstUnread !== null) {
      // The oldest loaded reply being unread means the first unread one may
      // be in an older page.
      if (replies[0]?.message.id !== firstUnread || !hasOlderReplies) {
        decide(firstUnread, UNREAD_REPLY_TOP_PX);
      } else if (canSeekOlder) {
        needsOlder = true;
      } else {
        decide(null);
      }
    } else if (remembered !== null) {
      if (has(remembered.messageId)) {
        decide(remembered.messageId, remembered.topPx);
      } else if (canSeekOlder) {
        needsOlder = true;
      } else {
        // Out of time keeps the reply; out of pages forgets it.
        decide(null, null, plan.seekExpired);
      }
    } else {
      decide(null, null, missed);
    }
  }

  // As in rooms: a visit that opens somewhere new forgets the old spot, but
  // one that ran out of time or read state keeps it for the next visit.
  const staleRemembered =
    plan?.decided === true &&
    plan.remembered !== null &&
    plan.targetId !== plan.remembered.messageId &&
    !plan.keptOnOpen;
  React.useEffect(() => {
    if (!staleRemembered || !plan?.scope) return;
    plan.remembered = null;
    readingPositionStore.set(plan.scope, plan.key, null);
  }, [plan, staleRemembered]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: tick re-checks a settled page
  React.useEffect(() => {
    if (!needsOlder || isFetchingOlder || !plan || plan.seekBusy || !fetchOlder)
      return;
    plan.seekStartedAt ??= Date.now();
    plan.pagesFetched += 1;
    plan.seekBusy = true;
    // A failed page is asked again after a pause rather than counted; the
    // deadline bounds it.
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

  const release = React.useCallback(() => {
    if (!plan || plan.released) return;
    plan.released = true;
    rerender();
  }, [plan]);

  /** Remembers where the reader is in this thread; null forgets it. */
  const savePosition = React.useCallback(
    (position: ReadingPosition | null) => {
      if (!plan?.scope) return;
      if (position === null && plan.keepsRemembered) return;
      plan.keepsRemembered = false;
      readingPositionStore.set(plan.scope, plan.key, position);
    },
    [plan],
  );

  const active = plan?.decided === true && !plan.released;
  return {
    targetId: active ? plan.targetId : null,
    topPx: active ? plan.topPx : null,
    isResolving: plan !== null && !plan.decided && !opensElsewhere,
    release,
    savePosition,
  };
}
