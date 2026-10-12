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
import { loadThreadReplies } from "@/features/messages/useThreadReplies";
import {
  getChannelWindowEvents,
  getChannelWindowsEvents,
} from "@/shared/api/channelWindow";
import { getEventById } from "@/shared/api/tauri";
import type { RelayEvent } from "@/shared/api/types";

const MESSAGES_STALE_MS = 5 * 60 * 1_000;
const MESSAGES_GC_MS = 60 * 60 * 1_000;
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

/**
 * Load a thread's replies in the background unless they are already cached.
 * Resolves to false when they were cached or the load failed.
 * `expectedEventId` is the reply the row opens on: a thread opened while this
 * load is in flight joins it, so it must enforce the same missing-reply retry.
 */
export async function preloadThreadReplies(
  queryClient: QueryClient,
  channelId: string,
  rootId: string,
  expectedEventId: string,
): Promise<boolean> {
  const queryKey = threadRepliesKey(channelId, rootId);
  // Opening a thread link resolves the reply and its root before the thread
  // shows; keep both at hand so that needs no relay round trip.
  const rememberRoot = rememberThreadRoot(queryClient, channelId, rootId);
  const cached = queryClient.getQueryData(queryKey) !== undefined;
  if (!cached) {
    await queryClient.prefetchQuery({
      queryKey,
      queryFn: () =>
        loadThreadReplies(queryClient, channelId, rootId, expectedEventId),
      retry: PRELOAD_RETRY,
      gcTime: MESSAGES_GC_MS,
    });
  }
  const replies = queryClient.getQueryData<RelayEvent[]>(queryKey);
  if (replies) rememberRouteEvents(queryClient, replies);
  await rememberRoot;
  return !cached && queryClient.getQueryState(queryKey)?.status === "success";
}

async function rememberThreadRoot(
  queryClient: QueryClient,
  channelId: string,
  rootId: string,
): Promise<void> {
  const cachedRoot = queryClient
    .getQueryData<RelayEvent[]>(channelMessagesKey(channelId))
    ?.find((event) => event.id === rootId);
  try {
    const root = cachedRoot ?? (await getEventById(rootId));
    if (root) rememberRouteEvents(queryClient, [root]);
  } catch (error) {
    console.warn("Failed to preload thread root", rootId, error);
  }
}
