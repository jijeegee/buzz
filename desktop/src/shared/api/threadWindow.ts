import {
  recordsStoreHead,
  requestWithStoreHeads,
} from "@/shared/api/relayStoreHeads";
import { invokeTauri } from "@/shared/api/tauri";
import type { RelayEvent, ThreadCursor } from "@/shared/api/types";

export type ThreadWindowRequest = {
  channelId: string;
  rootId: string;
  /** Oldest reply already loaded; null loads the newest page. */
  cursor: ThreadCursor | null;
};

export type ThreadWindowPage = {
  channelId: string;
  rootId: string;
  events: RelayEvent[];
  hasMore: boolean;
  /** Cursor for the next older page, when `hasMore`. */
  nextCursor: ThreadCursor | null;
  /** The bounds' paging direction (`older` for a newest page), when given. */
  direction?: string;
};

type RawThreadWindowPage = {
  channel_id: string;
  root_id: string;
  events: RelayEvent[];
  has_more: boolean;
  next_cursor: { created_at: number; event_id: string } | null;
  direction?: string;
};

/** The relay serves at most this many thread windows per request. */
export const MAX_THREAD_WINDOWS_PER_REQUEST = 4;

/**
 * Fetch newest-first reply pages for up to four threads in one relay round
 * trip. Threads the relay skipped (no access) are absent from the result.
 * With `storeHead` (the page a thread opens on, and the preload), each newest
 * page also records its thread's head span in the device store.
 */
export async function getThreadWindows(
  threads: readonly ThreadWindowRequest[],
  limitRows: number,
  options: { storeHead?: boolean } = {},
): Promise<ThreadWindowPage[]> {
  const heads = onceNewest(threads);
  if (heads.size === 0 || !recordsStoreHead(options, limitRows)) {
    return fetchThreadWindows(threads, limitRows);
  }
  const rooms = threads
    .filter((thread) => heads.has(headKey(thread)))
    .map((thread) => thread.channelId);
  return requestWithStoreHeads(
    [...new Set(rooms)],
    () => fetchThreadWindows(threads, limitRows),
    (pages) =>
      onceIn(pages)
        // A page is matched to its request by channel and root only, so a
        // root asked twice, or a page that is not a newest (`older`) one,
        // records nothing.
        .filter(
          (page) => heads.has(headKey(page)) && page.direction === "older",
        )
        .map((page) => ({
          roomId: page.channelId,
          rootId: page.rootId,
          hasMore: page.hasMore,
          nextCursor: page.nextCursor,
          events: page.events,
        })),
  );
}

function headKey(thread: { channelId: string; rootId: string }) {
  return `${thread.channelId}:${thread.rootId}`;
}

/** Keys of the threads asked once, for their newest page. */
function onceNewest(threads: readonly ThreadWindowRequest[]) {
  return new Set(
    onceIn(threads)
      .filter((thread) => thread.cursor === null)
      .map(headKey),
  );
}

/** The entries whose channel and root no other entry shares. */
function onceIn<T extends { channelId: string; rootId: string }>(
  entries: readonly T[],
): T[] {
  const counts = new Map<string, number>();
  for (const entry of entries) {
    counts.set(headKey(entry), (counts.get(headKey(entry)) ?? 0) + 1);
  }
  return entries.filter((entry) => counts.get(headKey(entry)) === 1);
}

async function fetchThreadWindows(
  threads: readonly ThreadWindowRequest[],
  limitRows: number,
): Promise<ThreadWindowPage[]> {
  const pages = await invokeTauri<RawThreadWindowPage[]>("get_thread_windows", {
    threads: threads.map((thread) => ({
      channelId: thread.channelId,
      rootId: thread.rootId,
      cursor: thread.cursor
        ? {
            created_at: thread.cursor.createdAt,
            event_id: thread.cursor.eventId,
          }
        : null,
    })),
    limitRows,
  });
  return pages.map((page) => ({
    channelId: page.channel_id,
    rootId: page.root_id,
    events: page.events,
    hasMore: page.has_more,
    nextCursor: page.next_cursor
      ? {
          createdAt: page.next_cursor.created_at,
          eventId: page.next_cursor.event_id,
        }
      : null,
    ...(page.direction === undefined ? {} : { direction: page.direction }),
  }));
}
