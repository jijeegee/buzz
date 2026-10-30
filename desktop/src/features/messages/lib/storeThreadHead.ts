/**
 * Opening a thread from the device store, paint first
 * (docs/message-sync.md#painting-from-the-store): the first load of a thread
 * reads its stored newest replies and asks the relay at the same time. Stored
 * replies that answer first are drawn at once, not yet confirmed, and the
 * thread is refetched right after, taking the relay request already out.
 * Later loads and older pages ask the relay.
 */
import type { QueryClient } from "@tanstack/react-query";

import { threadRepliesKey } from "@/features/messages/lib/messageQueryKeys";
import {
  forgetPaintCoverage,
  notePaintCoverage,
  storeScope,
  threadPaintKey,
} from "@/features/messages/lib/paintCoverage";
import { logStoreComparison } from "@/features/messages/lib/storeCompare";
import { storeOpensEnabled } from "@/features/messages/lib/storedWindows";
import { invokeTauri } from "@/shared/api/tauri";
import type { ThreadWindowPage } from "@/shared/api/threadWindow";
import type { RelayEvent } from "@/shared/api/types";

type RawStoredPage = {
  events: RelayEvent[];
  has_more: boolean;
  next_cursor: { created_at: number; event_id: string } | null;
};

/** A thread's stored page and from when its room is held without a gap. */
type RawThreadPaint = { window: RawStoredPage; complete_from: number | null };

type StoreThreadHeadDeps = {
  /**
   * The thread's newest stored replies, certain or not, read in the account
   * and server of `queryClient`; null when none.
   */
  threadPaint: (
    queryClient: QueryClient,
    channelId: string,
    rootId: string,
    limit: number,
  ) => Promise<RawThreadPaint | null>;
  /** Refetch the thread's query (it then reads the relay). */
  relayRefresh: (
    queryClient: QueryClient,
    channelId: string,
    rootId: string,
  ) => Promise<void>;
};

const defaultDeps: StoreThreadHeadDeps = {
  threadPaint: async (queryClient, channelId, rootId, limit) => {
    const scope = storeScope(queryClient);
    if (!storeOpensEnabled() || !scope) return null;
    return invokeTauri<RawThreadPaint | null>("message_store_thread_paint", {
      channelId,
      rootId,
      limit,
      account: scope.pubkey,
      relayUrl: scope.relayUrl,
    });
  },
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

/**
 * The thread's stored newest page to paint, with from when its room is held
 * without a gap, or `null` when none.
 */
export async function paintedThreadPage(
  queryClient: QueryClient,
  channelId: string,
  rootId: string,
  limit: number,
): Promise<{ page: ThreadWindowPage; completeFrom: number | null } | null> {
  const paint = await deps.threadPaint(queryClient, channelId, rootId, limit);
  const page = paint?.window;
  if (!page || !Array.isArray(page.events) || page.events.length === 0) {
    return null;
  }
  return {
    page: {
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
    },
    completeFrom: paint.complete_from ?? null,
  };
}

const threadKey = threadPaintKey;

type PendingPage = Promise<ThreadWindowPage | null>;

/** A painted thread's relay request, until its refetch takes it. */
let pendingRelays = new WeakMap<QueryClient, Map<string, PendingPage>>();

/** The relay request a painted thread's refetch takes, once. */
export function takePendingThreadRelay(
  queryClient: QueryClient,
  channelId: string,
  rootId: string,
): PendingPage | undefined {
  const pending = pendingRelays.get(queryClient);
  const key = threadKey(channelId, rootId);
  const page = pending?.get(key);
  pending?.delete(key);
  return page;
}

/** Threads drawn from a paint whose relay page has not been folded in yet. */
let unconfirmed = new WeakMap<QueryClient, Set<string>>();
const confirmationListeners = new Set<() => void>();

function setConfirmed(
  queryClient: QueryClient,
  channelId: string,
  rootId: string,
  confirmed: boolean,
) {
  let threads = unconfirmed.get(queryClient);
  if (!threads) {
    threads = new Set();
    unconfirmed.set(queryClient, threads);
  }
  const key = threadKey(channelId, rootId);
  if (threads.has(key) !== confirmed) return;
  if (confirmed) threads.delete(key);
  else threads.add(key);
  for (const listener of confirmationListeners) listener();
}

/** Whether the thread's replies are the relay's, not only a stored paint. */
export function isThreadConfirmed(
  queryClient: QueryClient,
  channelId: string,
  rootId: string,
): boolean {
  return !unconfirmed.get(queryClient)?.has(threadKey(channelId, rootId));
}

export function subscribeThreadConfirmations(listener: () => void) {
  confirmationListeners.add(listener);
  return () => {
    confirmationListeners.delete(listener);
  };
}

/**
 * The thread drew a stored paint; `relay` is its request still out. Without
 * one (it failed), the stored replies decide as they are.
 */
export function notePaintedThread(
  queryClient: QueryClient,
  channelId: string,
  rootId: string,
  relay: PendingPage | null,
  completeFrom: number | null,
) {
  setConfirmed(queryClient, channelId, rootId, false);
  if (!relay) return;
  notePaintCoverage(threadPaintKey(channelId, rootId), completeFrom);
  let pending = pendingRelays.get(queryClient);
  if (!pending) {
    pending = new Map();
    pendingRelays.set(queryClient, pending);
  }
  pending.set(threadKey(channelId, rootId), relay);
}

/** A relay page of the thread was folded in. */
export function noteThreadConfirmed(
  queryClient: QueryClient,
  channelId: string,
  rootId: string,
) {
  forgetPaintCoverage(threadPaintKey(channelId, rootId));
  setConfirmed(queryClient, channelId, rootId, true);
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
  logStoreComparison(`thread ${rootId.slice(0, 8)}`, "thread", stored, relay);
}

/** Forget the stored pages awaiting the relay (tests). */
export function resetStoredThreadPages() {
  storedAwaitingRelay = new WeakMap();
  pendingRelays = new WeakMap();
  unconfirmed = new WeakMap();
}

/**
 * The store only draws the first frame: once the fetch that took the stored
 * page has settled, the thread is refetched, which takes the relay request
 * already out (its window state is set); that load confirms the thread. A
 * thread closed meanwhile is not refetched; its request is then dropped and
 * the thread stays unconfirmed.
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
  takePendingThreadRelay(queryClient, channelId, rootId);
  // Settled either way: the replies held decide from here.
  forgetPaintCoverage(threadPaintKey(channelId, rootId));
}
