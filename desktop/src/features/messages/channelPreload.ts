import { CONVERSATION_CACHE_GC_MS } from "@/shared/api/queryClient";
import type { QueryClient } from "@tanstack/react-query";

import { reconcileFetchedChannelWindow } from "@/features/messages/hooks";
import { markStoreRoomHeadFetched } from "@/features/messages/lib/storeRoomHead";
import {
  channelHeadCacheScope,
  channelHeadHydration,
  markPaintableChannelSnapshot,
} from "@/features/messages/lib/channelHeadCache";
import type { ChannelWindowStore } from "@/features/messages/lib/channelWindowStore";
import {
  channelMessagesKey,
  channelWindowKey,
  threadRepliesKey,
} from "@/features/messages/lib/messageQueryKeys";
import { rememberRouteEvents } from "@/features/messages/lib/routeEventCache";
import {
  planStorePreload,
  storePreloadSummary,
} from "@/features/messages/lib/storePreloadPlan";
import {
  markStoredWindow,
  storeOpenLiveAt,
  storeOpensEnabled,
} from "@/features/messages/lib/storedWindows";
import { seedThreadPages } from "@/features/messages/lib/threadWindowCache";
import { getChannelWindowsEvents } from "@/shared/api/channelWindow";
import {
  STORE_LIVE_WAIT_MS,
  waitStoreLive,
} from "@/shared/api/relayStoreCoverage";
import { getEventsByIds, invokeTauri } from "@/shared/api/tauri";
import {
  getThreadWindows,
  MAX_THREAD_WINDOWS_PER_REQUEST,
} from "@/shared/api/threadWindow";
import type { RelayEvent } from "@/shared/api/types";

const MESSAGES_STALE_MS = 5 * 60 * 1_000;
const MESSAGES_GC_MS = CONVERSATION_CACHE_GC_MS;
/** Background work retries once; the scheduler retries later passes. */
const PRELOAD_RETRY = 1;

/** A room whose newest stored window is certain, with that window. */
type CertainRoom = { roomId: string; events: RelayEvent[] };

type StorePreloadDeps = {
  /** Whether the store may vouch for anything: store opens on, clock trusted. */
  trusted: () => Promise<boolean>;
  /** Resolves once the rooms' live REQs have settled, at most 3 s. */
  waitLive: (roomIds: readonly string[], signal?: AbortSignal) => Promise<void>;
  /** When the room's live REQ reached EOSE (its token), or null. */
  liveAt: (roomId: string) => number | null;
  /** The rooms whose newest stored window is certain, with those windows. */
  certain: (
    rooms: { roomId: string; liveAt: number }[],
  ) => Promise<CertainRoom[]>;
};

const defaultStoreDeps: StorePreloadDeps = {
  trusted: async () =>
    storeOpensEnabled() &&
    (await invokeTauri<boolean>("message_store_clock_trusted").catch(
      () => false,
    )),
  waitLive: (roomIds, signal) =>
    waitStoreLive(roomIds, STORE_LIVE_WAIT_MS, signal),
  liveAt: storeOpenLiveAt,
  certain: (rooms) =>
    invokeTauri<CertainRoom[]>("message_store_certain_rooms", { rooms }),
};

let storeDeps = defaultStoreDeps;

/** Replace the store bridge (tests). Returns the restore function. */
export function setStorePreloadDeps(next: Partial<StorePreloadDeps>) {
  const previous = storeDeps;
  storeDeps = { ...storeDeps, ...next };
  return () => {
    storeDeps = previous;
  };
}

export type PreloadChannelsOptions = {
  /** Aborted when the pass is cancelled (sign-out, account switch, unmount). */
  signal?: AbortSignal;
  /** Rooms the store held certainly, and that were drawn from it. */
  onCertain?: (roomIds: string[]) => void;
  /** Runs right before the relay request; false skips it (user busy). */
  beforeFetch?: () => Promise<boolean>;
};

function sameScope(
  left: ReturnType<typeof channelHeadCacheScope>,
  right: ReturnType<typeof channelHeadCacheScope>,
) {
  return left?.pubkey === right?.pubkey && left?.relayUrl === right?.relayUrl;
}

/**
 * Of `rooms`, those that need the relay: once their live REQs have settled
 * (at most 3 s), rooms whose newest stored window is certain are drawn from
 * the store as a paintable copy, not marked relay-fetched (opening them
 * reads the store again and revalidates once), and left out
 * (docs/message-sync.md#store-first-preload). Null when the pass ended.
 */
async function roomsNeedingRelay(
  queryClient: QueryClient,
  rooms: readonly string[],
  options: PreloadChannelsOptions,
): Promise<string[] | null> {
  const { signal } = options;
  // Untrusted: nothing can be certain, so nothing to wait for.
  const trusted = await storeDeps.trusted();
  if (trusted) await storeDeps.waitLive(rooms, signal);
  if (signal?.aborted) return null;
  // A room opened meanwhile loads itself.
  const cold = rooms.filter(
    (roomId) =>
      queryClient.getQueryData(channelMessagesKey(roomId)) === undefined,
  );
  const liveAts = new Map<string, number>();
  for (const roomId of trusted ? cold : []) {
    const liveAt = storeDeps.liveAt(roomId);
    if (liveAt !== null) liveAts.set(roomId, liveAt);
  }
  const tokens = new Set(liveAts.keys());
  const check = planStorePreload(cold, tokens, new Set()).check;
  const certain =
    check.length === 0
      ? []
      : await storeDeps
          .certain(
            check.map((roomId) => ({
              roomId,
              liveAt: liveAts.get(roomId) ?? 0,
            })),
          )
          .catch(() => []);
  if (signal?.aborted) return null;
  const windows = new Map(certain.map((room) => [room.roomId, room.events]));
  const plan = planStorePreload(cold, tokens, new Set(windows.keys()));
  console.info(storePreloadSummary(plan));
  for (const roomId of plan.certain) {
    const events = windows.get(roomId);
    if (!events) continue;
    if (queryClient.getQueryData(channelMessagesKey(roomId)) !== undefined) {
      continue;
    }
    markStoredWindow(events);
    await storeChannelPage(queryClient, roomId, async () => events, false);
  }
  if (plan.certain.length > 0) options.onCertain?.(plan.certain);
  return plan.fetch;
}

/**
 * Load the newest page of several rooms in one relay round trip, so opening
 * them paints at once. Rooms that already have messages cached are not
 * reloaded (see `keepCachedChannel`); opening still revalidates as usual.
 * Rooms the store holds certainly are drawn from it instead
 * (`roomsNeedingRelay`). Resolves to the rooms loaded from the relay.
 */
export async function preloadChannelsMessages(
  queryClient: QueryClient,
  channelIds: readonly string[],
  limitRows: number,
  options: PreloadChannelsOptions = {},
): Promise<string[]> {
  await channelHeadHydration(queryClient);
  const scope = channelHeadCacheScope(queryClient);
  const cold = channelIds.filter(
    (channelId) => !keepCachedChannel(queryClient, channelId),
  );
  if (cold.length === 0) return [];
  const fetch = await roomsNeedingRelay(queryClient, cold, options);
  if (!fetch || fetch.length === 0) return [];
  if (options.beforeFetch && !(await options.beforeFetch())) return [];
  // Signed out, switched account or left: nothing more goes out.
  if (
    options.signal?.aborted ||
    !sameScope(scope, channelHeadCacheScope(queryClient))
  ) {
    return [];
  }
  let pages: Map<string, RelayEvent[]>;
  try {
    pages = await getChannelWindowsEvents(fetch, limitRows, {
      storeHead: true,
    });
  } catch (error) {
    // The scheduler retries in a later pass; a burst of single requests here
    // would not yield to the user.
    console.warn("Batched room preload failed", error);
    return [];
  }
  const loaded: string[] = [];
  for (const channelId of fetch) {
    const events = pages.get(channelId);
    // A room the relay skipped stays cold; opening it loads as usual.
    if (!events) continue;
    if (queryClient.getQueryData(channelMessagesKey(channelId)) !== undefined) {
      continue;
    }
    if (await storeChannelPage(queryClient, channelId, async () => events)) {
      loaded.push(channelId);
    }
  }
  return loaded;
}

/**
 * A room that already has messages cached (open, visited, or restored from the
 * persisted head) is not reloaded, but it may paint that copy while opening
 * revalidates it, so opening never holds a skeleton over known rows.
 */
function keepCachedChannel(queryClient: QueryClient, channelId: string) {
  const cached = queryClient.getQueryData<unknown[]>(
    channelMessagesKey(channelId),
  );
  if (cached === undefined) return false;
  // Live events or an optimistic send can seed this cache before any relay
  // page lands; only a relay-delivered newest page is safe to paint.
  const newestPage = queryClient
    .getQueryData<ChannelWindowStore>(channelWindowKey(channelId))
    ?.pages.find((page) => page.startCursor === null);
  if (cached.length > 0 && (newestPage?.rows.length ?? 0) > 0) {
    markPaintableChannelSnapshot(queryClient, channelId);
  }
  return true;
}

async function storeChannelPage(
  queryClient: QueryClient,
  channelId: string,
  loadEvents: () => Promise<RelayEvent[]>,
  fromRelay = true,
): Promise<boolean> {
  const queryKey = channelMessagesKey(channelId);
  await queryClient.prefetchQuery({
    queryKey,
    queryFn: async ({ signal }) =>
      reconcileFetchedChannelWindow(
        queryClient,
        channelId,
        await loadEvents(),
        [],
        signal,
        false,
      ),
    retry: PRELOAD_RETRY,
    staleTime: MESSAGES_STALE_MS,
    gcTime: MESSAGES_GC_MS,
  });
  if (queryClient.getQueryState(queryKey)?.status !== "success") return false;
  if (fromRelay) {
    // A relay page: the next fetch of this room asks the relay, not the store.
    markStoreRoomHeadFetched(queryClient, channelId);
  } else {
    // A store window paints at once; opening still fetches (from the store,
    // then the relay once), as a first open would.
    await queryClient.invalidateQueries({
      queryKey,
      exact: true,
      refetchType: "none",
    });
  }
  // An empty page holds the skeleton on open, as an empty persisted head does.
  if ((queryClient.getQueryData<unknown[]>(queryKey)?.length ?? 0) === 0) {
    return false;
  }
  markPaintableChannelSnapshot(queryClient, channelId);
  return true;
}

/** Newest replies loaded per preloaded thread, about one panel. */
export const THREAD_PRELOAD_REPLIES = 20;

export type PreloadThread = { channelId: string; rootId: string };

/**
 * Load the newest replies of several threads in the background, four per relay
 * round trip, and keep each reply and thread root at hand so opening a thread
 * link needs no relay round trip. Threads already cached are not reloaded.
 * `beforeRequest` runs before each request with the threads it carries and
 * stops the preload when it resolves to false (the user is busy or the pass
 * was cancelled).
 */
export async function preloadThreads(
  queryClient: QueryClient,
  threads: readonly PreloadThread[],
  beforeRequest: (
    threads: readonly PreloadThread[],
  ) => Promise<boolean> = async () => true,
): Promise<number> {
  let loaded = 0;
  const cold = threads.filter(
    (thread) =>
      queryClient.getQueryData(
        threadRepliesKey(thread.channelId, thread.rootId),
      ) === undefined,
  );
  for (
    let start = 0;
    start < cold.length;
    start += MAX_THREAD_WINDOWS_PER_REQUEST
  ) {
    const chunk = cold.slice(start, start + MAX_THREAD_WINDOWS_PER_REQUEST);
    if (!(await beforeRequest(chunk))) return loaded;
    try {
      const pages = await getThreadWindows(
        chunk.map((thread) => ({ ...thread, cursor: null })),
        THREAD_PRELOAD_REPLIES,
        // Narrower than the page a thread opens on: records no head span.
        { storeHead: true },
      );
      seedThreadPages(queryClient, pages);
      loaded += pages.length;
    } catch (error) {
      console.warn("Failed to preload threads", error);
    }
  }
  for (const thread of threads) {
    const replies = queryClient.getQueryData<RelayEvent[]>(
      threadRepliesKey(thread.channelId, thread.rootId),
    );
    if (replies) rememberRouteEvents(queryClient, replies);
  }
  if (await beforeRequest([])) await rememberThreadRoots(queryClient, threads);
  return loaded;
}

async function rememberThreadRoots(
  queryClient: QueryClient,
  threads: readonly PreloadThread[],
): Promise<void> {
  const missing: string[] = [];
  for (const { channelId, rootId } of threads) {
    const cachedRoot = queryClient
      .getQueryData<RelayEvent[]>(channelMessagesKey(channelId))
      ?.find((event) => event.id === rootId);
    if (cachedRoot) {
      rememberRouteEvents(queryClient, [cachedRoot]);
    } else if (!missing.includes(rootId)) {
      missing.push(rootId);
    }
  }
  if (missing.length === 0) return;
  try {
    rememberRouteEvents(queryClient, await getEventsByIds(missing));
  } catch (error) {
    console.warn("Failed to preload thread roots", error);
  }
}
