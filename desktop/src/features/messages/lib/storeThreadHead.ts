/**
 * Opening a thread from the device store: when the store's newest replies
 * are certain (docs/message-sync.md#opening-a-thread-from-the-store), the
 * first load of a thread takes them instead of waiting for the relay, and the
 * relay is asked right after. Later loads and older pages ask the relay.
 */
import type { QueryClient } from "@tanstack/react-query";

import { threadRepliesKey } from "@/features/messages/lib/messageQueryKeys";
import { logStoreComparison } from "@/features/messages/lib/storeCompare";
import { storeOpenLiveAt } from "@/features/messages/lib/storedWindows";
import { invokeTauri } from "@/shared/api/tauri";
import type { ThreadWindowPage } from "@/shared/api/threadWindow";
import type { RelayEvent } from "@/shared/api/types";

type RawStoredPage = {
  events: RelayEvent[];
  has_more: boolean;
  next_cursor: { created_at: number; event_id: string } | null;
};

type StoreThreadHeadDeps = {
  liveAt: (roomId: string) => number | null;
  threadHead: (
    channelId: string,
    rootId: string,
    limit: number,
    liveAt: number,
  ) => Promise<RawStoredPage | null>;
  /** Refetch the thread's query (it then reads the relay). */
  relayRefresh: (
    queryClient: QueryClient,
    channelId: string,
    rootId: string,
  ) => Promise<void>;
};

const defaultDeps: StoreThreadHeadDeps = {
  liveAt: storeOpenLiveAt,
  threadHead: (channelId, rootId, limit, liveAt) =>
    invokeTauri<RawStoredPage | null>("message_store_thread_head", {
      channelId,
      rootId,
      limit,
      liveAt,
    }),
  relayRefresh: (queryClient, channelId, rootId) =>
    queryClient.invalidateQueries({
      queryKey: threadRepliesKey(channelId, rootId),
      exact: true,
    }),
};

let deps = defaultDeps;

/** Replace the store bridge (tests). Returns the restore function. */
export function setStoreThreadHeadDeps(next: Partial<StoreThreadHeadDeps>) {
  const previous = deps;
  deps = { ...deps, ...next };
  return () => {
    deps = previous;
  };
}

/** The thread's certain stored newest page, or `null` (then ask the relay). */
export async function certainStoredThreadPage(
  channelId: string,
  rootId: string,
  limit: number,
): Promise<ThreadWindowPage | null> {
  const liveAt = deps.liveAt(channelId);
  if (liveAt === null) return null;
  try {
    const page = await deps.threadHead(channelId, rootId, limit, liveAt);
    if (!page || !Array.isArray(page.events)) return null;
    return {
      channelId,
      rootId,
      events: page.events,
      hasMore: page.has_more,
      nextCursor: page.next_cursor
        ? {
            createdAt: page.next_cursor.created_at,
            eventId: page.next_cursor.event_id,
          }
        : null,
    };
  } catch {
    return null;
  }
}

/** Stored pages awaiting the relay's, per thread, to compare them. */
let storedAwaitingRelay = new WeakMap<QueryClient, Map<string, RelayEvent[]>>();

function awaitingIn(queryClient: QueryClient) {
  let threads = storedAwaitingRelay.get(queryClient);
  if (!threads) {
    threads = new Map();
    storedAwaitingRelay.set(queryClient, threads);
  }
  return threads;
}

/**
 * Compares a relay page of `rootId`'s newest replies with the stored page it
 * replaces, if one is waiting. Only an actual relay answer is compared.
 */
export function compareThreadRelayPage(
  queryClient: QueryClient,
  rootId: string,
  relay: readonly RelayEvent[],
) {
  const threads = awaitingIn(queryClient);
  const stored = threads.get(rootId);
  if (!stored) return;
  threads.delete(rootId);
  logStoreComparison(`thread ${rootId.slice(0, 8)}`, stored, relay);
}

/** Forget the stored pages awaiting the relay (tests). */
export function resetStoredThreadPages() {
  storedAwaitingRelay = new WeakMap();
}

/**
 * The store only draws the first frame: once the fetch that took the stored
 * page has settled, the thread is refetched, which now reads the relay (its
 * window state is set).
 */
export async function refreshThreadAfterStore(
  queryClient: QueryClient,
  channelId: string,
  rootId: string,
  stored: readonly RelayEvent[],
) {
  awaitingIn(queryClient).set(rootId, [...stored]);
  await Promise.resolve();
  const queryKey = threadRepliesKey(channelId, rootId);
  const query = queryClient.getQueryCache().find({ queryKey, exact: true });
  await query?.promise?.catch(() => {});
  try {
    await deps.relayRefresh(queryClient, channelId, rootId);
  } catch (error) {
    console.error("Failed to refresh a thread opened from the store", error);
  }
}
