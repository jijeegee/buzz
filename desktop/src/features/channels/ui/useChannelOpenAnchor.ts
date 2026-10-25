import * as React from "react";

import { hasPendingLandAtLatest } from "@/features/channels/lib/landAtLatest";
import {
  type ReadingPosition,
  readingPositionStore,
} from "@/features/channels/readingPositionStore";
import { getThreadReference } from "@/features/messages/lib/threading";
import { relayClient } from "@/shared/api/relayClient";
import type { RelayEvent } from "@/shared/api/types";

/**
 * Older pages fetched from the server while looking for the first unread
 * before giving up and opening at the newest message (about 200 top-level
 * rows at 50 per page).
 */
export const MAX_UNREAD_SEEK_PAGES = 4;
/** Older pages read from the local store for the same seek (2000 rows). */
export const MAX_STORED_SEEK_PAGES = 40;
/** A failed seek page is tried again after this, behind the loading state. */
export const SEEK_RETRY_MS = 8_000;

/** The relay socket is not connected: the server cannot answer now. */
export const isRelayOffline = () =>
  relayClient.getConnectionState() !== "connected";

/** Where one older page came from, or why none was appended. */
export type OlderPageOutcome = "store" | "server" | "failed" | "busy" | "none";

type OpenPlan = {
  channelId: string;
  decided: boolean;
  anchorMessageId: string | null;
  /** Where the anchor's row sits when it is a remembered position. */
  anchorTopPx: number | null;
  serverPages: number;
  storedPages: number;
  /** A seek page is being fetched, or waits for its retry. */
  seeking: boolean;
  retryTimer: ReturnType<typeof setTimeout> | null;
  /** The seek cannot go further: offline, or no older page came. */
  stopped: boolean;
  disposed: boolean;
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
  fetchOlderPage: () => Promise<OlderPageOutcome>;
  /** Whether the relay is unreachable now (read when a seek page fails). */
  isOffline: () => boolean;
  /** The visit opens somewhere else (a linked message, a huddle transcript). */
  opensElsewhere: boolean;
};

/**
 * Decides once per channel visit where the main timeline opens: at the first
 * unread message when there is one, otherwise where the reader left the
 * channel, otherwise at the newest message. When the row to open at may be
 * older than the loaded ones, older pages are fetched first while the
 * timeline keeps its loading state; the list then mounts directly at its
 * final position. Pages come from the local store where it holds them (up to
 * MAX_STORED_SEEK_PAGES), else the server (up to MAX_UNREAD_SEEK_PAGES); a
 * failed page is retried after SEEK_RETRY_MS. Offline, the seek stops and the
 * room opens at the newest stored row, where the unread pill offers the rest.
 */
export function useChannelOpenAnchor({
  channelId,
  pubkey,
  messages,
  frontierSeconds,
  firstUnreadMessageId,
  hasOlderMessages,
  isFetchingOlder,
  fetchOlderPage,
  isOffline,
  opensElsewhere,
}: UseChannelOpenAnchorOptions) {
  const planRef = React.useRef<OpenPlan | null>(null);
  // Bumped when a seek page settles, so the seek effect runs again.
  const [seekTick, rerender] = React.useReducer(
    (count: number) => count + 1,
    0,
  );
  if (channelId === null) {
    planRef.current = null;
  } else if (planRef.current?.channelId !== channelId) {
    planRef.current = {
      channelId,
      decided: false,
      anchorMessageId: null,
      anchorTopPx: null,
      serverPages: 0,
      storedPages: 0,
      seeking: false,
      retryTimer: null,
      stopped: false,
      disposed: false,
      remembered: pubkey ? readingPositionStore.get(pubkey, channelId) : null,
      landsAtLatest: hasPendingLandAtLatest(channelId),
    };
  }
  const plan = planRef.current;

  let needsOlder = false;
  if (plan && !plan.decided && messages !== undefined) {
    const decide = (
      anchorMessageId: string | null,
      anchorTopPx: number | null = null,
    ) => {
      plan.decided = true;
      plan.anchorMessageId = anchorMessageId;
      plan.anchorTopPx = anchorMessageId === null ? null : anchorTopPx;
    };
    const canSeekOlder =
      hasOlderMessages &&
      !plan.stopped &&
      plan.serverPages < MAX_UNREAD_SEEK_PAGES &&
      plan.storedPages < MAX_STORED_SEEK_PAGES;
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
        // Unread for too long, or offline: the start is out of reach. The
        // newest row opens, with the unread pill pointing up.
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

  // biome-ignore lint/correctness/useExhaustiveDependencies: seekTick re-runs the seek after a page settles
  React.useEffect(() => {
    if (!needsOlder || isFetchingOlder || !plan || plan.seeking) return;
    plan.seeking = true;
    void fetchOlderPage().then((outcome) => {
      if (planRef.current !== plan || plan.disposed) return;
      if (outcome === "failed" && !isOffline()) {
        plan.retryTimer = setTimeout(() => {
          plan.retryTimer = null;
          plan.seeking = false;
          rerender();
        }, SEEK_RETRY_MS);
        return;
      }
      plan.seeking = false;
      if (outcome === "store") plan.storedPages += 1;
      else if (outcome === "server") plan.serverPages += 1;
      // Offline, or nothing was appended: stop seeking rather than ask again.
      else if (outcome === "failed" || outcome === "none") plan.stopped = true;
      // "busy": another older load runs; its end re-runs this effect.
      if (outcome !== "busy") rerender();
    });
  }, [fetchOlderPage, isFetchingOlder, isOffline, needsOlder, plan, seekTick]);

  React.useEffect(() => {
    if (!plan) return;
    plan.disposed = false;
    return () => {
      plan.disposed = true;
      if (plan.retryTimer) clearTimeout(plan.retryTimer);
    };
  }, [plan]);

  return {
    anchorMessageId: plan?.decided ? plan.anchorMessageId : null,
    anchorTopPx: plan?.decided ? plan.anchorTopPx : null,
    isResolving: plan !== null && !plan.decided && messages !== undefined,
  };
}
