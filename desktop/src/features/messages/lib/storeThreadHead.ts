/**
 * Opening a thread from the device store: when the store's newest replies
 * are certain (docs/message-sync.md#opening-a-thread-from-the-store), the
 * first load of a thread takes them instead of waiting for the relay. Later
 * loads and older pages still ask the relay.
 */
import { storeLiveAt } from "@/shared/api/relayStoreCoverage";
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
};

const defaultDeps: StoreThreadHeadDeps = {
  liveAt: storeLiveAt,
  threadHead: (channelId, rootId, limit, liveAt) =>
    invokeTauri<RawStoredPage | null>("message_store_thread_head", {
      channelId,
      rootId,
      limit,
      liveAt,
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
