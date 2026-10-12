import type { QueryClient } from "@tanstack/react-query";

import { reconcileFetchedChannelWindow } from "@/features/messages/hooks";
import {
  channelHeadHydration,
  markPaintableChannelSnapshot,
} from "@/features/messages/lib/channelHeadCache";
import {
  channelMessagesKey,
  threadRepliesKey,
} from "@/features/messages/lib/messageQueryKeys";
import { loadThreadReplies } from "@/features/messages/useThreadReplies";
import { getChannelWindowEvents } from "@/shared/api/channelWindow";

const MESSAGES_STALE_MS = 5 * 60 * 1_000;
const MESSAGES_GC_MS = 60 * 60 * 1_000;
/** Background work retries once; the scheduler retries later passes. */
const PRELOAD_RETRY = 1;

/**
 * Load a room's newest page in the background so opening it paints at once.
 * Rooms that already have messages cached (open, visited, or restored from the
 * persisted head) are left alone. Opening still revalidates as usual.
 * Resolves to false when the room was skipped or the load failed.
 */
export async function preloadChannelMessages(
  queryClient: QueryClient,
  channelId: string,
  limitRows: number,
): Promise<boolean> {
  await channelHeadHydration(queryClient);
  const queryKey = channelMessagesKey(channelId);
  if (queryClient.getQueryData(queryKey) !== undefined) return false;
  await queryClient.prefetchQuery({
    queryKey,
    queryFn: async ({ signal }) =>
      reconcileFetchedChannelWindow(
        queryClient,
        channelId,
        await getChannelWindowEvents(channelId, null, limitRows),
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
  if (queryClient.getQueryData(queryKey) !== undefined) return false;
  await queryClient.prefetchQuery({
    queryKey,
    queryFn: () =>
      loadThreadReplies(queryClient, channelId, rootId, expectedEventId),
    retry: PRELOAD_RETRY,
    gcTime: MESSAGES_GC_MS,
  });
  return queryClient.getQueryState(queryKey)?.status === "success";
}
