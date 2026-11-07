/**
 * How room and thread pages go through the window check
 * (docs/message-sync.md#window-check): today's request, what its answer
 * holds, and the stored window in the same shape.
 */
import type { PageRows, WindowCursor, WindowIo } from "./windowCheck";
import { getChannelWindowEvents } from "@/shared/api/channelWindow";
import {
  recordsStoreHead,
  roomWindowAnswer,
} from "@/shared/api/relayStoreHeads";
import {
  getThreadWindows,
  type ThreadWindowPage,
} from "@/shared/api/threadWindow";
import type { RelayEvent } from "@/shared/api/types";
import {
  CHANNEL_WINDOW_ROW_KINDS,
  KIND_CHANNEL_WINDOW_BOUNDS,
  KIND_STREAM_MESSAGE,
  KIND_STREAM_MESSAGE_V2,
} from "@/shared/constants/kinds";

const ROOM_ROW_KINDS: ReadonlySet<number> = new Set(CHANNEL_WINDOW_ROW_KINDS);
/** The kinds a thread window pages (the relay's and the store's). */
const THREAD_ROW_KINDS: ReadonlySet<number> = new Set([
  KIND_STREAM_MESSAGE,
  KIND_STREAM_MESSAGE_V2,
]);

/** A room page's rows and whether more remain; null without one bounds. */
export function roomPageRows(events: readonly RelayEvent[]): PageRows | null {
  const bounds = events.filter(
    (event) => event.kind === KIND_CHANNEL_WINDOW_BOUNDS,
  );
  if (bounds.length !== 1) return null;
  let hasMore: unknown;
  try {
    hasMore = (JSON.parse(bounds[0].content) as { has_more?: unknown })
      .has_more;
  } catch {
    return null;
  }
  if (typeof hasMore !== "boolean") return null;
  const others = events.filter((event) => event !== bounds[0]);
  return {
    ids: others.map((event) => event.id),
    rows: others.filter((event) => ROOM_ROW_KINDS.has(event.kind)),
    hasMore,
  };
}

/** A thread page's replies (never its root) and overlays. */
export function threadPageRows(
  rootId: string,
  page: ThreadWindowPage | null,
): PageRows | null {
  if (!page) return null;
  return {
    ids: page.events.map((event) => event.id),
    rows: page.events.filter(
      (event) => THREAD_ROW_KINDS.has(event.kind) && event.id !== rootId,
    ),
    hasMore: page.hasMore,
  };
}

/** A room's window below `cursor` (newest when null). */
export function roomWindowIo(
  roomId: string,
  cursor: WindowCursor | null,
  limit: number,
  options: { storeHead?: boolean } = {},
): WindowIo<RelayEvent[]> {
  const records = cursor === null && recordsStoreHead(options, limit);
  return {
    fetch: () => getChannelWindowEvents(roomId, cursor, limit, options),
    page: roomPageRows,
    // The store answers a room window in the relay's shape, bounds included.
    fromStore: (stored) => stored.events,
    fromCheckPage: (events) => events,
    ...(records
      ? { head: (events: RelayEvent[]) => roomWindowAnswer(roomId, events) }
      : {}),
  };
}

/** A thread's replies below `cursor` (newest when null). */
export function threadWindowIo(
  channelId: string,
  rootId: string,
  cursor: WindowCursor | null,
  limit: number,
  options: { storeHead?: boolean } = {},
): WindowIo<ThreadWindowPage | null> {
  const records = cursor === null && recordsStoreHead(options, limit);
  return {
    fetch: async (includeAux) => {
      const [page] = await getThreadWindows(
        [{ channelId, rootId, cursor }],
        limit,
        includeAux ? { ...options, includeAux } : options,
      );
      return page ?? null;
    },
    page: (page) => threadPageRows(rootId, page),
    fromStore: (stored) => ({
      channelId,
      rootId,
      events: stored.events,
      hasMore: stored.hasMore,
      nextCursor: stored.hasMore ? stored.oldest : null,
      direction: "older",
    }),
    ...(records
      ? {
          head: (page: ThreadWindowPage | null) =>
            page
              ? {
                  roomId: channelId,
                  rootId,
                  hasMore: page.hasMore,
                  nextCursor: page.nextCursor,
                  events: page.events,
                }
              : null,
        }
      : {}),
  };
}
