import type { QueryClient } from "@tanstack/react-query";

import type { getChannelWindowEvents } from "@/shared/api/channelWindow";
import { storeChannelHeadCache } from "@/shared/api/tauriChannelHeadCache";
import type { RelayEvent } from "@/shared/api/types";
import { channelMessagesKey, channelWindowKey } from "./messageQueryKeys";
import {
  emptyChannelWindowStore,
  replaceNewestChannelWindow,
  type ChannelWindowStore,
} from "./channelWindowStore";
import { parseChannelWindowResponse } from "./channelWindowResponse";
import { reconcileChannelWindowMessages } from "./channelWindowReconciliation";
import {
  channelHeadCacheScope,
  channelHeadHydration,
} from "./channelHeadCache";
import { markRenderScopedReactionsLoaded } from "./renderScopedReactions";
import {
  isPaintedWindow,
  isStoredWindow,
  storeFollowUp,
} from "./storedWindows";
import {
  noteRoomWindowCommitted,
  type RoomWindowListener,
  windowFetchedLive,
} from "./windowLiveToken";

/**
 * When each room last committed a fetched window (relay, or a certain one from
 * the device store), per query cache. Seeded head caches, the snapshot fetch
 * that consumes them, stored paints and live merges never count: they can
 * miss newer rows.
 */
const windowCommits = new WeakMap<QueryClient, Map<string, number>>();
const commitListeners = new Set<() => void>();

/** Calls `listener` after any room commits a fetched window. */
export function subscribeChannelWindowCommits(listener: () => void) {
  commitListeners.add(listener);
  return () => {
    commitListeners.delete(listener);
  };
}

/** The room's last fetched window: when, and its newest row's time. */
export function channelWindowCommit(
  queryClient: QueryClient,
  channelId: string | null,
): {
  at: number;
  newestRowAt: number | null;
  newestRowId: string | null;
} | null {
  const at = channelId ? windowCommits.get(queryClient)?.get(channelId) : null;
  if (!channelId || at == null) return null;
  const window = queryClient.getQueryData<ChannelWindowStore>(
    channelWindowKey(channelId),
  );
  const newestRow = window?.pages[0]?.rows[0]?.event;
  return {
    at,
    newestRowAt: newestRow?.created_at ?? null,
    newestRowId: newestRow?.id ?? null,
  };
}

/** Keep the rendered timeline cache aligned with its authoritative window. */
export function projectChannelWindowMessages(
  queryClient: QueryClient,
  channelId: string,
) {
  const window =
    queryClient.getQueryData<ChannelWindowStore>(channelWindowKey(channelId)) ??
    emptyChannelWindowStore();
  queryClient.setQueryData<RelayEvent[]>(
    channelMessagesKey(channelId),
    (messages = []) => reconcileChannelWindowMessages(window, messages),
  );
}

export async function refreshChannelWindowMessages(
  queryClient: QueryClient,
  channelId: string,
  joinStoreFollowUp = true,
  // The open's room listener, when this is the refresh after it went live.
  liveListener?: RoomWindowListener,
) {
  const queryKey = channelMessagesKey(channelId);
  // Sequence behind persisted-head hydration. While the channel query is parked
  // on that gate it has no data, so TanStack would dedupe this invalidation
  // onto it — and that fetch returns the seeded snapshot, never asking the
  // relay. A seeded query is recognisable by data at `dataUpdatedAt` 0; let its
  // snapshot fetch settle (consuming the mount gate) before invalidating, so
  // the refetch is a distinct authoritative window fetch. Concurrent callers
  // (subscribe settlement + reconnect) wake on the same promise, so the seeded
  // branch must join an authoritative fetch already in flight rather than
  // cancel and replace it. Cold and warm channels carry no such marker and
  // dedupe/cancel exactly as before.
  await channelHeadHydration(queryClient);
  const query = queryClient.getQueryCache().find({ queryKey, exact: true });
  const seeded =
    query?.state.data !== undefined && query.state.dataUpdatedAt === 0;
  if (seeded) {
    await query.promise?.catch(() => {});
  }
  // A room just opened from the store is already asking the relay.
  const followUp = joinStoreFollowUp
    ? storeFollowUp(queryClient, channelId)
    : undefined;
  if (followUp) return followUp;
  // The open's own request already went out with its listener live.
  const fetching = query?.state.fetchStatus === "fetching";
  const live =
    liveListener !== undefined &&
    (await windowFetchedLive(
      queryClient,
      channelId,
      liveListener,
      fetching ? query?.promise : undefined,
    ));
  // A store open that settled while this waited is asking the relay now.
  const joined = joinStoreFollowUp
    ? storeFollowUp(queryClient, channelId)
    : undefined;
  if (joined) return joined;
  if (!live) {
    await queryClient.invalidateQueries(
      { queryKey, exact: true, refetchType: "active" },
      { cancelRefetch: !seeded, throwOnError: true },
    );
  }
  projectChannelWindowMessages(queryClient, channelId);
}

export function reconcileFetchedChannelWindow(
  queryClient: QueryClient,
  channelId: string,
  events: Awaited<ReturnType<typeof getChannelWindowEvents>>,
  previousMessages: RelayEvent[],
  signal: AbortSignal,
  // Background loads must not rank as visits in the persisted-head LRU.
  persistHead = true,
): RelayEvent[] {
  // Tauri invokes cannot be canceled after dispatch. A replacement refetch can
  // therefore win while this older request is still in flight. Never let that
  // canceled request commit its stale page into the authoritative window.
  signal.throwIfAborted();
  const windowKey = channelWindowKey(channelId);
  const page = parseChannelWindowResponse(events, channelId, null);
  markRenderScopedReactionsLoaded(
    channelId,
    page.rows.map((row) => row.event.id),
  );
  const current =
    queryClient.getQueryData<ChannelWindowStore>(windowKey) ??
    emptyChannelWindowStore();
  const next = replaceNewestChannelWindow(current, page);
  queryClient.setQueryData(windowKey, next);
  noteRoomWindowCommitted(events);
  // A stored paint is drawn but not fetched: the relay's answer commits.
  if (!isPaintedWindow(events)) {
    let commits = windowCommits.get(queryClient);
    if (!commits) {
      commits = new Map();
      windowCommits.set(queryClient, commits);
    }
    commits.set(channelId, Date.now());
    for (const listener of commitListeners) listener();
  }
  // A window from the device store is not persisted; the relay's answer
  // that follows is.
  const scope =
    persistHead && !isStoredWindow(events)
      ? channelHeadCacheScope(queryClient)
      : null;
  if (scope) {
    void storeChannelHeadCache(scope, channelId, events).catch((error) => {
      console.warn("Failed to persist channel head", channelId, error);
    });
  }
  return reconcileChannelWindowMessages(next, previousMessages);
}
