import * as React from "react";

import {
  type ReadingPosition,
  readingPositionStore,
} from "@/features/channels/readingPositionStore";
import { useAppShell } from "@/app/AppShellContext";
import {
  reanchorTarget,
  threadPaintDecides,
} from "@/features/channels/lib/openAnchorDecision";
import {
  paintCoverage,
  threadPaintKey,
} from "@/features/messages/lib/paintCoverage";
import type { MainTimelineEntry } from "@/features/messages/lib/threadPanel";

import {
  MAX_UNREAD_SEEK_PAGES,
  UNREAD_SEEK_DEADLINE_MS,
  UNREAD_SEEK_PAGE_SIZE,
  UNREAD_SEEK_RETRY_MS,
} from "./useChannelOpenAnchor";
import type { ThreadOpenRead } from "./useChannelUnreadState";
import { useReaderMoved } from "./useReaderMoved";

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
  /**
   * Decided on stored replies the relay has not confirmed yet: decided again
   * once it has, moving if the reader has not scrolled.
   */
  provisional: boolean;
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
  /** The replies are the relay's, not only a stored paint waiting for it. */
  repliesConfirmed?: boolean;
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
 * nothing is drawn elsewhere first. Stored replies decide at once, but
 * provisionally: once the relay's replies are in, the open moves if the reader
 * has not scrolled (docs/message-sync.md#opening-at-a-position).
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
  repliesConfirmed = true,
  hasOlderReplies,
  isFetchingOlder,
  fetchOlder,
}: UseThreadOpenTargetOptions) {
  const [tick, rerender] = React.useReducer((count: number) => count + 1, 0);
  const { isReadStateSettled } = useAppShell();
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
      provisional: false,
    };
  }
  const plan = planRef.current;

  let needsOlder = false;
  const readerMoved = React.useRef(false);
  const waitsForReadState = readState === "pending" && !opensElsewhere;
  const deciding = plan !== null && !plan.decided;
  const rechecking =
    plan?.decided === true && plan.provisional && repliesConfirmed;
  if (plan && (deciding || rechecking) && !isLoading && !waitsForReadState) {
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
    let decision:
      | { targetId: string | null; topPx: number | null; keeps: boolean }
      | "seek";
    if (opensElsewhere) {
      decision = { targetId: null, topPx: null, keeps: false };
    } else if (firstUnread !== null) {
      // The oldest loaded reply being unread means the first unread one may
      // be in an older page.
      if (replies[0]?.message.id !== firstUnread || !hasOlderReplies) {
        decision = {
          targetId: firstUnread,
          topPx: UNREAD_REPLY_TOP_PX,
          keeps: false,
        };
      } else if (canSeekOlder) {
        decision = "seek";
      } else {
        decision = { targetId: null, topPx: null, keeps: false };
      }
    } else if (remembered !== null) {
      if (has(remembered.messageId)) {
        decision = {
          targetId: remembered.messageId,
          topPx: remembered.topPx,
          keeps: false,
        };
      } else if (canSeekOlder) {
        decision = "seek";
      } else {
        // Out of time keeps the reply; out of pages forgets it.
        decision = { targetId: null, topPx: null, keeps: plan.seekExpired };
      }
    } else {
      decision = { targetId: null, topPx: null, keeps: missed };
    }
    const firstUnreadIndex =
      firstUnread === null
        ? -1
        : replies.findIndex((reply) => reply.message.id === firstUnread);
    const paintDecides = threadPaintDecides(
      channelId && rootId
        ? paintCoverage(threadPaintKey(channelId, rootId))
        : undefined,
      firstUnreadIndex < 0 || opensElsewhere
        ? null
        : replies[firstUnreadIndex].message.createdAt,
      firstUnreadIndex > 0
        ? replies[firstUnreadIndex - 1].message.createdAt
        : null,
      hasOlderReplies,
    );
    if (rechecking) {
      if (isReadStateSettled) plan.provisional = false;
      const next = reanchorTarget(
        plan.targetId,
        decision === "seek"
          ? { kind: "seek" }
          : {
              kind: "open",
              anchorMessageId: decision.targetId,
              anchorTopPx: decision.topPx,
              keepsRemembered: decision.keeps,
            },
        readerMoved.current,
      );
      if (next?.kind === "open") {
        plan.targetId = next.anchorMessageId;
        plan.topPx = next.anchorMessageId === null ? null : next.anchorTopPx;
        plan.released = false;
        plan.keptOnOpen = false;
        plan.keepsRemembered = false;
      } else if (next?.kind === "seek") {
        // The relay's replies put the open in an older page: find it first.
        plan.decided = false;
        plan.provisional = false;
        needsOlder = true;
      }
    } else if (!repliesConfirmed && !paintDecides) {
      // Stored replies with a hole before the first unread one: an earlier
      // unread reply may be missing, so the relay's replies decide.
    } else if (decision === "seek") {
      needsOlder = true;
    } else {
      plan.decided = true;
      plan.provisional = !repliesConfirmed || !isReadStateSettled;
      plan.targetId = decision.targetId;
      plan.topPx = decision.targetId === null ? null : decision.topPx;
      plan.keptOnOpen = decision.keeps && plan.remembered !== null;
      plan.keepsRemembered = plan.keptOnOpen;
    }
  }

  // As in rooms: a visit that opens somewhere new forgets the old spot, but
  // one that ran out of time or read state keeps it for the next visit.
  useReaderMoved(readerMoved, plan?.decided === true && plan.provisional, key);
  const staleRemembered =
    plan?.decided === true &&
    !plan.provisional &&
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
