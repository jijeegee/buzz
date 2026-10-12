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
};

type RawThreadWindowPage = {
  channel_id: string;
  root_id: string;
  events: RelayEvent[];
  has_more: boolean;
  next_cursor: { created_at: number; event_id: string } | null;
};

/** The relay serves at most this many thread windows per request. */
export const MAX_THREAD_WINDOWS_PER_REQUEST = 4;

/**
 * Fetch newest-first reply pages for up to four threads in one relay round
 * trip. Threads the relay skipped (no access) are absent from the result.
 */
export async function getThreadWindows(
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
  }));
}
