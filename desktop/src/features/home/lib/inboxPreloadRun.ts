import {
  INBOX_PRELOAD_ROWS,
  type InboxPreloadTarget,
} from "@/features/home/lib/inboxPreload";
import type { PreloadThread } from "@/features/messages/channelPreload";

/** Rooms one background-wave request carries (mobile `maxChannelWindowsPerRequest`). */
export const INBOX_PRELOAD_ROOMS_PER_REQUEST = 12;

export type InboxPreloadRunDeps = {
  /** Gates the first wave; false stops the pass. */
  waitStart: () => Promise<boolean>;
  /** Gates each request of the first wave's threads. */
  waitFirstThreads: () => Promise<boolean>;
  /** Gates each background-wave request. */
  waitIdle: () => Promise<boolean>;
  /**
   * Preloads rooms; `beforeFetch` gates the relay request once the store
   * check is done (the background wave re-checks that the user is idle).
   */
  preloadRooms: (
    channelIds: string[],
    beforeFetch?: () => Promise<boolean>,
  ) => Promise<unknown>;
  preloadThreads: (
    threads: PreloadThread[],
    beforeRequest: (threads: readonly PreloadThread[]) => Promise<boolean>,
  ) => Promise<unknown>;
};

function roomsOf(targets: readonly InboxPreloadTarget[]) {
  return [...new Set(targets.map((target) => target.channelId))];
}

function threadsOf(targets: readonly InboxPreloadTarget[]): PreloadThread[] {
  return targets.flatMap((target) =>
    target.thread
      ? [{ channelId: target.channelId, rootId: target.thread.rootId }]
      : [],
  );
}

/**
 * Two waves, as mobile's `runInboxPreload`: the rooms behind the top
 * {@link INBOX_PRELOAD_ROWS} rows in one request and their threads four per
 * request; then the rest of the rows' rooms,
 * {@link INBOX_PRELOAD_ROOMS_PER_REQUEST} per request, and their threads,
 * each request waiting until `waitIdle` resolves true.
 */
export async function runInboxPreload(
  targets: readonly InboxPreloadTarget[],
  deps: InboxPreloadRunDeps,
): Promise<void> {
  const first = targets.slice(0, INBOX_PRELOAD_ROWS);
  const rest = targets.slice(INBOX_PRELOAD_ROWS);

  if (!(await deps.waitStart())) return;
  const firstRooms = roomsOf(first);
  if (firstRooms.length > 0) await deps.preloadRooms(firstRooms);
  const firstThreads = threadsOf(first);
  if (firstThreads.length > 0) {
    await deps.preloadThreads(firstThreads, deps.waitFirstThreads);
  }

  const loaded = new Set(firstRooms);
  const rooms = roomsOf(rest).filter((roomId) => !loaded.has(roomId));
  for (
    let start = 0;
    start < rooms.length;
    start += INBOX_PRELOAD_ROOMS_PER_REQUEST
  ) {
    if (!(await deps.waitIdle())) return;
    await deps.preloadRooms(
      rooms.slice(start, start + INBOX_PRELOAD_ROOMS_PER_REQUEST),
      deps.waitIdle,
    );
  }
  const restThreads = threadsOf(rest);
  if (restThreads.length > 0) {
    await deps.preloadThreads(restThreads, deps.waitIdle);
  }
}
