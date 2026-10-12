import { CONVERSATION_CACHE_GC_MS } from "@/shared/api/queryClient";
import type { QueryClient } from "@tanstack/react-query";

import { reconcileFetchedChannelWindow } from "@/features/messages/hooks";
import {
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
import { seedThreadPages } from "@/features/messages/lib/threadWindowCache";
import {
  getChannelWindowEvents,
  getChannelWindowsEvents,
} from "@/shared/api/channelWindow";
import { getEventsByIds } from "@/shared/api/tauri";
import {
  getThreadWindows,
  MAX_THREAD_WINDOWS_PER_REQUEST,
} from "@/shared/api/threadWindow";
import type { RelayEvent } from "@/shared/api/types";

const MESSAGES_STALE_MS = 5 * 60 * 1_000;
const MESSAGES_GC_MS = CONVERSATION_CACHE_GC_MS;
/** Background work retries once; the scheduler retries later passes. */
const PRELOAD_RETRY = 1;

/**
 * Load a room's newest page in the background so opening it paints at once.
 * Rooms that already have messages cached are not reloaded (see
 * `keepCachedChannel`). Opening still revalidates as usual.
 * Resolves to false when the room was skipped or the load failed.
 */
export async function preloadChannelMessages(
  queryClient: QueryClient,
  channelId: string,
  limitRows: number,
): Promise<boolean> {
  await channelHeadHydration(queryClient);
  if (keepCachedChannel(queryClient, channelId)) return false;
  return storeChannelPage(queryClient, channelId, () =>
    getChannelWindowEvents(channelId, null, limitRows),
  );
}

/**
 * Load the newest page of several rooms in one relay round trip. Resolves to
 * the rooms loaded.
 */
export async function preloadChannelsMessages(
  queryClient: QueryClient,
  channelIds: readonly string[],
  limitRows: number,
): Promise<string[]> {
  await channelHeadHydration(queryClient);
  const cold = channelIds.filter(
    (channelId) => !keepCachedChannel(queryClient, channelId),
  );
  if (cold.length === 0) return [];
  let pages: Map<string, RelayEvent[]>;
  try {
    pages = await getChannelWindowsEvents(cold, limitRows);
  } catch (error) {
    // The scheduler retries in a later pass; a burst of single requests here
    // would not yield to the user.
    console.warn("Batched room preload failed", error);
    return [];
  }
  const loaded: string[] = [];
  for (const channelId of cold) {
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
