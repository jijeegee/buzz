import type { QueryClient } from "@tanstack/react-query";
import type { RelayEvent } from "@/shared/api/types";
import { selectThreadName } from "./threadName";

export const THREAD_NAME_QUERY_ROOT = "thread-name";

export const threadNameQueryKey = (channelId: string, threadId: string) => [
  THREAD_NAME_QUERY_ROOT,
  channelId,
  threadId,
];

const pendingKey = (channelId: string, threadId: string) =>
  `${channelId}\u0000${threadId}`;

/**
 * Live renames that arrived while their key's first read was in flight. The
 * read folds them in (`mergeThreadName`), so the read's older answer never
 * replaces them.
 */
const pending = new Map<string, RelayEvent>();

function tagValue(event: RelayEvent, name: string) {
  return event.tags.find((tag) => tag[0] === name)?.[1] ?? null;
}

/**
 * Apply one live thread name (40009 from the room bundle's metadata) to the
 * cache. Only rows the app holds are touched: a key with data is updated
 * (newest wins), a key still loading keeps the event for its read, and any
 * other key is ignored.
 */
export function applyLiveThreadName(
  queryClient: QueryClient,
  event: RelayEvent,
) {
  const channelId = tagValue(event, "h");
  const threadId = tagValue(event, "e");
  if (!channelId || !threadId) return;
  const key = threadNameQueryKey(channelId, threadId);
  const state = queryClient.getQueryState<RelayEvent | null>(key);
  if (!state) return;
  if (state.data === undefined) {
    const id = pendingKey(channelId, threadId);
    const next = selectThreadName(
      pending.get(id) ?? null,
      event,
      channelId,
      threadId,
    );
    if (next) pending.set(id, next);
    return;
  }
  queryClient.setQueryData<RelayEvent | null>(key, (head) =>
    selectThreadName(head ?? null, event, channelId, threadId),
  );
}

/**
 * A read's answer merged with what the cache holds and any live rename that
 * arrived during the read: newest wins, never built from nothing (B2).
 */
export function mergeThreadName(
  queryClient: QueryClient,
  channelId: string,
  threadId: string,
  fetched: RelayEvent | null,
): RelayEvent | null {
  const id = pendingKey(channelId, threadId);
  const live = pending.get(id) ?? null;
  pending.delete(id);
  let head =
    queryClient.getQueryData<RelayEvent | null>(
      threadNameQueryKey(channelId, threadId),
    ) ?? null;
  for (const event of [fetched, live]) {
    if (event) head = selectThreadName(head, event, channelId, threadId);
  }
  return head;
}

export type ThreadNameLiveDeps = {
  listenRoomMetadata: (listener: (event: RelayEvent) => void) => () => void;
  subscribeToReconnects: (listener: () => void) => () => void;
};

/**
 * One cache-wide listener for live thread names, for every row on screen in
 * every member room, instead of one REQ per row. A reconnect re-reads the
 * names on screen (active queries), since renames during the gap are not
 * replayed. Returns the function that stops it.
 */
export function startThreadNameLive(
  queryClient: QueryClient,
  deps: ThreadNameLiveDeps,
) {
  const stopNames = deps.listenRoomMetadata((event) =>
    applyLiveThreadName(queryClient, event),
  );
  const stopReconnects = deps.subscribeToReconnects(() => {
    void queryClient.invalidateQueries({
      queryKey: [THREAD_NAME_QUERY_ROOT],
    });
  });
  return () => {
    stopNames();
    stopReconnects();
  };
}
