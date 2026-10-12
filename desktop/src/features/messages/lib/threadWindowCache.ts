import type { QueryClient } from "@tanstack/react-query";

import {
  dedupeMessagesById,
  sortMessages,
  threadRepliesKey,
} from "@/features/messages/lib/messageQueryKeys";
import {
  getThreadWindows,
  type ThreadWindowPage,
} from "@/shared/api/threadWindow";
import type { RelayEvent, ThreadCursor } from "@/shared/api/types";

/** Replies a thread opens with: about one panel of conversation. */
export const THREAD_PAGE_ROWS = 50;
/** Older pages walked to reach a reply a link points at before giving up. */
const MAX_TARGET_PAGES = 10;

/**
 * How far a thread's cached replies reach. Replies load newest first:
 * `olderCursor` is the oldest loaded reply (the start of the next older page)
 * and `newestCovered` the newest reply a relay read has vouched for, so a later
 * newest page can tell whether it connects to what is cached.
 */
export type ThreadWindowState = {
  hasOlder: boolean;
  olderCursor: ThreadCursor | null;
  newestCovered: ThreadCursor | null;
};

export function threadWindowStateKey(channelId: string, rootId: string) {
  return ["thread-window-state", channelId, rootId] as const;
}

/** Kinds a thread window pages; everything else in the cache rides along. */
const THREAD_ROW_KINDS: ReadonlySet<number> = new Set([9, 40002, 45001, 45003]);

function isReplyRow(event: RelayEvent) {
  return THREAD_ROW_KINDS.has(event.kind);
}

function cursorOf(event: RelayEvent): ThreadCursor {
  return { createdAt: event.created_at, eventId: event.id };
}

/** Relay order is newest first, then ascending id within a second. */
function isOlderThan(event: RelayEvent, cursor: ThreadCursor) {
  return (
    event.created_at < cursor.createdAt ||
    (event.created_at === cursor.createdAt && event.id > cursor.eventId)
  );
}

function isNewerThan(event: RelayEvent, cursor: ThreadCursor) {
  return !isOlderThan(event, cursor) && event.id !== cursor.eventId;
}

function oldestOf(events: readonly RelayEvent[]): ThreadCursor | null {
  let oldest: RelayEvent | null = null;
  for (const event of events) {
    if (!oldest || isOlderThan(event, cursorOf(oldest))) oldest = event;
  }
  return oldest ? cursorOf(oldest) : null;
}

function newestOf(events: readonly RelayEvent[]): ThreadCursor | null {
  let newest: RelayEvent | null = null;
  for (const event of events) {
    if (!newest || isNewerThan(event, cursorOf(newest))) newest = event;
  }
  return newest ? cursorOf(newest) : null;
}

/** True when `cursor` sits at or before `bound` in the thread. */
function reachesBack(cursor: ThreadCursor, bound: ThreadCursor) {
  return (
    cursor.createdAt < bound.createdAt ||
    (cursor.createdAt === bound.createdAt && cursor.eventId >= bound.eventId)
  );
}

function olderState(
  page: ThreadWindowPage,
): Pick<ThreadWindowState, "hasOlder" | "olderCursor"> {
  return page.hasMore && page.nextCursor
    ? { hasOlder: true, olderCursor: page.nextCursor }
    : { hasOlder: false, olderCursor: null };
}

/**
 * Fold a freshly fetched newest page into the cached replies.
 *
 * Only reply rows decide spans: the page is authoritative for the rows it
 * covers, so cached rows inside that span but missing from the page (deleted
 * since) drop out. Older cached rows are kept only when the page reaches back
 * to the newest row an earlier relay read covered; otherwise replies may have
 * been missed in between, so the cache restarts from the page. Rows newer than
 * the page (live replies that landed while it was in flight) and non-row
 * events (reactions, edits, deletions written by live updates) are kept.
 */
export function mergeNewestThreadPage(
  cached: readonly RelayEvent[] | undefined,
  cachedState: ThreadWindowState | undefined,
  page: ThreadWindowPage,
): { events: RelayEvent[]; state: ThreadWindowState } {
  const pageIds = new Set(page.events.map((event) => event.id));
  const pageRows = page.events.filter(isReplyRow);
  const pageOldest = oldestOf(pageRows);
  const pageNewest = newestOf(pageRows);
  const previous = (cached ?? []).filter((event) => !pageIds.has(event.id));
  const previousRows = previous.filter(isReplyRow);
  const carried = previous.filter((event) => !isReplyRow(event));
  const newerRows = pageNewest
    ? previousRows.filter((event) => isNewerThan(event, pageNewest))
    : previousRows;
  const keep = [...page.events, ...newerRows, ...carried];
  if (!pageOldest || !page.hasMore) {
    // The page holds the whole thread.
    return {
      events: sortMessages(keep),
      state: { hasOlder: false, olderCursor: null, newestCovered: pageNewest },
    };
  }
  const covered = cachedState?.newestCovered ?? null;
  if (!covered || !reachesBack(pageOldest, covered)) {
    return {
      events: sortMessages(keep),
      state: { ...olderState(page), newestCovered: pageNewest },
    };
  }
  const olderRows = previousRows.filter((event) =>
    isOlderThan(event, pageOldest),
  );
  return {
    events: sortMessages([...olderRows, ...keep]),
    state: {
      hasOlder: cachedState?.hasOlder ?? false,
      olderCursor: cachedState?.olderCursor ?? null,
      newestCovered: pageNewest,
    },
  };
}

/** Fold an older page (fetched from `olderCursor`) into the cached replies. */
export function mergeOlderThreadPage(
  cached: readonly RelayEvent[] | undefined,
  cachedState: ThreadWindowState | undefined,
  page: ThreadWindowPage,
): { events: RelayEvent[]; state: ThreadWindowState } {
  const events = sortMessages(
    dedupeMessagesById([...page.events, ...(cached ?? [])]),
  );
  return {
    events,
    state: {
      ...olderState(page),
      newestCovered:
        cachedState?.newestCovered ?? newestOf(events.filter(isReplyRow)),
    },
  };
}

function sameCursor(left: ThreadCursor | null, right: ThreadCursor | null) {
  return (
    left === right ||
    (left !== null &&
      right !== null &&
      left.createdAt === right.createdAt &&
      left.eventId === right.eventId)
  );
}

async function fetchThreadPage(
  channelId: string,
  rootId: string,
  cursor: ThreadCursor | null,
  limitRows: number,
): Promise<ThreadWindowPage | null> {
  const [page] = await getThreadWindows(
    [{ channelId, rootId, cursor }],
    limitRows,
  );
  return page ?? null;
}

/**
 * Load a thread's newest page and fold it into the cache. When `mustInclude`
 * is given (a link opens on that reply), older pages are walked until it is
 * loaded. Resolves to null when the relay serves no window for this thread
 * (root outside the channel, or no access); callers fall back to the full read.
 */
export async function loadNewestThreadReplies(
  queryClient: QueryClient,
  channelId: string,
  rootId: string,
  options: { limitRows?: number; mustInclude?: string | null } = {},
): Promise<RelayEvent[] | null> {
  const page = await fetchThreadPage(
    channelId,
    rootId,
    null,
    options.limitRows ?? THREAD_PAGE_ROWS,
  );
  if (!page) return null;
  const stateKey = threadWindowStateKey(channelId, rootId);
  let { events, state } = mergeNewestThreadPage(
    queryClient.getQueryData<RelayEvent[]>(threadRepliesKey(channelId, rootId)),
    queryClient.getQueryData<ThreadWindowState>(stateKey),
    page,
  );
  const target = options.mustInclude;
  for (
    let walked = 0;
    target &&
    walked < MAX_TARGET_PAGES &&
    state.hasOlder &&
    state.olderCursor &&
    !events.some((event) => event.id === target);
    walked += 1
  ) {
    const older = await fetchThreadPage(
      channelId,
      rootId,
      state.olderCursor,
      THREAD_PAGE_ROWS,
    );
    if (!older) break;
    ({ events, state } = mergeOlderThreadPage(events, state, older));
  }
  // A link whose reply was not reached keeps the cache as it was: saving the
  // walked-past cursor without the walked replies would hide them for good.
  if (!target || events.some((event) => event.id === target)) {
    queryClient.setQueryData(stateKey, state);
  }
  return events;
}

/** Load the page before the oldest cached reply into the thread cache. */
export async function loadOlderThreadReplies(
  queryClient: QueryClient,
  channelId: string,
  rootId: string,
): Promise<void> {
  const stateKey = threadWindowStateKey(channelId, rootId);
  const state = queryClient.getQueryData<ThreadWindowState>(stateKey);
  if (!state?.hasOlder || !state.olderCursor) return;
  const page = await fetchThreadPage(
    channelId,
    rootId,
    state.olderCursor,
    THREAD_PAGE_ROWS,
  );
  if (!page) return;
  // A newest-page reload may have restarted the cache meanwhile; this page
  // then no longer borders what is cached.
  const current = queryClient.getQueryData<ThreadWindowState>(stateKey);
  if (!current || !sameCursor(current.olderCursor, state.olderCursor)) return;
  const repliesKey = threadRepliesKey(channelId, rootId);
  const merged = mergeOlderThreadPage(
    queryClient.getQueryData<RelayEvent[]>(repliesKey),
    current,
    page,
  );
  queryClient.setQueryData(repliesKey, merged.events);
  queryClient.setQueryData(stateKey, merged.state);
}

/**
 * Seed thread caches from batched newest pages (background preload). Threads
 * already cached are left to their own revalidation.
 */
export function seedThreadPages(
  queryClient: QueryClient,
  pages: readonly ThreadWindowPage[],
): void {
  for (const page of pages) {
    const repliesKey = threadRepliesKey(page.channelId, page.rootId);
    if (queryClient.getQueryData(repliesKey) !== undefined) continue;
    const { events, state } = mergeNewestThreadPage(undefined, undefined, page);
    queryClient.setQueryData(repliesKey, events);
    queryClient.setQueryData(
      threadWindowStateKey(page.channelId, page.rootId),
      state,
    );
  }
}
