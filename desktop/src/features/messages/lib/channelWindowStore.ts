import type { RelayEvent } from "@/shared/api/types";
import { isMatchingPendingMessage } from "./messageMerge";

export type ChannelWindowCursor = { createdAt: number; eventId: string };
export type ChannelWindowThreadSummary = {
  replyCount: number;
  descendantCount: number;
  lastReplyAt: number | null;
  participantPubkeys: string[];
};
export type ChannelWindowRow = {
  event: RelayEvent;
  thread: ChannelWindowThreadSummary | null;
};
export type LiveThreadSummary = {
  summary: ChannelWindowThreadSummary;
  /** `created_at` of the relay 39005 that carried it — newest wins per root. */
  createdAt: number;
};
export type ChannelWindowPage = {
  startCursor: ChannelWindowCursor | null;
  rows: ChannelWindowRow[];
  aux: RelayEvent[];
  nextCursor: ChannelWindowCursor | null;
  hasMore: boolean;
};
export type ChannelWindowStore = {
  pages: ChannelWindowPage[];
  /** Top-level live events not represented in an authoritative relay page. */
  liveOverlay: RelayEvent[];
  /** Live structural events retained independently from frozen page closure. */
  liveAux: RelayEvent[];
  /**
   * Relay-pushed 39005 summaries (keyed by thread root id) not yet superseded
   * by a page. A page response supersedes them for the roots it re-delivers.
   */
  liveSummaries: Record<string, LiveThreadSummary>;
};

export const emptyChannelWindowStore = (): ChannelWindowStore => ({
  pages: [],
  liveOverlay: [],
  liveAux: [],
  liveSummaries: {},
});

function cursorsEqual(
  left: ChannelWindowCursor | null,
  right: ChannelWindowCursor | null,
) {
  return (
    left === right ||
    (left !== null &&
      right !== null &&
      left.createdAt === right.createdAt &&
      left.eventId === right.eventId)
  );
}

/** Relay order: newest timestamp first, then ascending id within a second. */
export function compareRelayOrder(left: RelayEvent, right: RelayEvent) {
  return left.created_at !== right.created_at
    ? right.created_at - left.created_at
    : left.id < right.id
      ? -1
      : left.id > right.id
        ? 1
        : 0;
}

function isStrictlyOlder(event: RelayEvent, cursor: ChannelWindowCursor) {
  return (
    event.created_at < cursor.createdAt ||
    (event.created_at === cursor.createdAt && event.id > cursor.eventId)
  );
}

function assertValidPage(page: ChannelWindowPage) {
  if (page.hasMore !== (page.nextCursor !== null)) {
    throw new Error("Channel window hasMore and nextCursor disagree.");
  }
  const seen = new Set<string>();
  for (let index = 0; index < page.rows.length; index += 1) {
    const event = page.rows[index].event;
    if (seen.has(event.id))
      throw new Error(`Duplicate channel row ${event.id}.`);
    seen.add(event.id);
    if (page.startCursor && !isStrictlyOlder(event, page.startCursor)) {
      throw new Error(
        `Channel row ${event.id} is outside its cursor interval.`,
      );
    }
    const previous = page.rows[index - 1]?.event;
    if (previous && compareRelayOrder(previous, event) > 0) {
      throw new Error("Couldn't load messages. Try again.");
    }
  }
}

function targetsAny(event: RelayEvent, ids: Set<string>) {
  return event.tags.some((tag) => tag[0] === "e" && ids.has(tag[1]));
}

/**
 * The retained rows older than a new newest page, re-cut into one page that
 * starts at its end cursor, or null when there are none or the two do not
 * join. They join when a retained row is not older than the new page's end
 * cursor: the old chain then runs unbroken from inside the new page down to
 * its own tail. A new page with no more history proves every older row gone.
 * Cases: test-fixtures/channel-window-replace.json.
 */
function retainedOlderPage(
  current: ChannelWindowStore,
  page: ChannelWindowPage,
): ChannelWindowPage | null {
  const end = page.nextCursor;
  const tail = current.pages[current.pages.length - 1];
  const newest = current.pages[0]?.rows[0]?.event;
  if (!end || !tail || !newest || isStrictlyOlder(newest, end)) return null;
  const rows = current.pages
    .flatMap((retained) => retained.rows)
    .filter((row) => isStrictlyOlder(row.event, end));
  if (rows.length === 0) return null;
  // Overlays on the kept rows, and deletions of those overlays.
  const targets = new Set(rows.map((row) => row.event.id));
  const pageAuxIds = new Set(page.aux.map((event) => event.id));
  const candidates = current.pages
    .flatMap((retained) => retained.aux)
    .filter(
      (event, index, all) =>
        !pageAuxIds.has(event.id) &&
        all.findIndex((other) => other.id === event.id) === index,
    );
  const aux: RelayEvent[] = [];
  for (let hop = 0; hop < 2; hop += 1) {
    for (const event of candidates) {
      if (!aux.includes(event) && targetsAny(event, targets)) aux.push(event);
    }
    for (const event of aux) targets.add(event.id);
  }
  aux.sort(
    (left, right) => candidates.indexOf(left) - candidates.indexOf(right),
  );
  return {
    startCursor: end,
    rows,
    aux,
    nextCursor: tail.nextCursor,
    hasMore: tail.hasMore,
  };
}

/**
 * Replace the authoritative chain at page zero. Rows inside the new page's
 * span are its rows (those it no longer has were deleted); older retained
 * rows that join it stay, re-cut so the cursor chain stays continuous, which
 * keeps an anchor found by paging back (docs/message-sync.md#opening-at-a-position).
 */
export function replaceNewestChannelWindow(
  current: ChannelWindowStore,
  page: ChannelWindowPage,
): ChannelWindowStore {
  if (page.startCursor !== null) {
    throw new Error("Newest channel page must have a null start cursor.");
  }
  assertValidPage(page);
  const older = retainedOlderPage(current, page);
  const pages = older ? [page, older] : [page];
  const ids = new Set(
    pages.flatMap((retained) => retained.rows.map((row) => row.event.id)),
  );
  const auxIds = new Set(page.aux.map((event) => event.id));
  const pageRootIds = new Set(page.rows.map((row) => row.event.id));
  const liveOverlay = current.liveOverlay.filter((event) => !ids.has(event.id));
  const shownRootIds = new Set([
    ...(older?.rows ?? []).map((row) => row.event.id),
    ...liveOverlay.map((event) => event.id),
  ]);
  return {
    pages,
    liveOverlay,
    liveAux: current.liveAux.filter((event) => !auxIds.has(event.id)),
    // A head refetch is the authoritative resync moment (subscribe/reconnect
    // both funnel here): live summaries for the roots it re-delivers yield to
    // its snapshot; those for kept older rows and overlay rows stay, and the
    // rest go with their rows.
    liveSummaries: Object.fromEntries(
      Object.entries(current.liveSummaries).filter(
        ([rootId]) => !pageRootIds.has(rootId) && shownRootIds.has(rootId),
      ),
    ),
  };
}

/** Append only a response that continues the retained echoed cursor chain. */
export function appendOlderChannelWindow(
  current: ChannelWindowStore,
  page: ChannelWindowPage,
): ChannelWindowStore {
  assertValidPage(page);
  const tail = current.pages[current.pages.length - 1];
  if (!tail) throw new Error("Load the newest channel page first.");
  if (!tail.hasMore || !tail.nextCursor) {
    throw new Error("The channel window is already complete.");
  }
  if (!cursorsEqual(page.startCursor, tail.nextCursor)) {
    throw new Error(
      "Channel page does not continue the retained cursor chain.",
    );
  }
  const ids = new Set(
    current.pages.flatMap((retained) =>
      retained.rows.map((row) => row.event.id),
    ),
  );
  for (const row of page.rows) {
    if (ids.has(row.event.id)) {
      throw new Error(`Channel row ${row.event.id} overlaps a retained page.`);
    }
  }
  const pageIds = new Set(page.rows.map((row) => row.event.id));
  return {
    ...current,
    pages: [...current.pages, page],
    liveOverlay: current.liveOverlay.filter((event) => !pageIds.has(event.id)),
  };
}

/**
 * Record a relay-pushed live `39005` summary. Newest `created_at` wins per
 * root: the relay pushes a full recount on every thread mutation, so the
 * latest push is authoritative for that root — including counting *down*
 * after a delete. Retained across scrollback pages (a racing push can be
 * fresher than a just-fetched page summary) and cleared only by a head
 * refetch that re-delivers the root (`replaceNewestChannelWindow`).
 */
export function mergeLiveThreadSummary(
  current: ChannelWindowStore,
  rootId: string,
  incoming: LiveThreadSummary,
): ChannelWindowStore {
  const existing = current.liveSummaries[rootId];
  if (existing && existing.createdAt >= incoming.createdAt) return current;
  return {
    ...current,
    liveSummaries: { ...current.liveSummaries, [rootId]: incoming },
  };
}

/**
 * Merge a live top-level event without mutating authoritative page boundaries.
 * Events below an open oldest boundary wait for ordinary relay pagination.
 */
export function mergeLiveChannelWindowEvent(
  current: ChannelWindowStore,
  event: RelayEvent,
  isTimelineRow = true,
): ChannelWindowStore {
  if (!isTimelineRow) {
    if (
      current.liveAux.some((candidate) => candidate.id === event.id) ||
      current.pages.some((page) =>
        page.aux.some((candidate) => candidate.id === event.id),
      )
    ) {
      return current;
    }
    return { ...current, liveAux: [...current.liveAux, event] };
  }
  if (
    current.pages.some((page) =>
      page.rows.some((row) => row.event.id === event.id),
    )
  ) {
    return current;
  }
  const oldestPage = current.pages[current.pages.length - 1];
  const oldest = oldestPage?.rows[oldestPage.rows.length - 1]?.event;
  if (
    oldest &&
    (event.created_at < oldest.created_at ||
      (oldestPage.hasMore && compareRelayOrder(event, oldest) >= 0))
  ) {
    return current;
  }
  // An echo of an own send that was marked failed (it timed out after the
  // relay stored it) takes that row's place and render key.
  const failedRow = event.pending
    ? undefined
    : current.liveOverlay.find(
        (candidate) =>
          candidate.failed === true &&
          isMatchingPendingMessage(candidate, event),
      );
  const incoming = failedRow
    ? { ...event, localKey: failedRow.localKey ?? failedRow.id }
    : event;
  return {
    ...current,
    liveOverlay: current.liveOverlay
      .filter(
        (candidate) => candidate.id !== event.id && candidate !== failedRow,
      )
      .concat(incoming)
      .sort(compareRelayOrder),
  };
}

/**
 * Apply a per-event transform across every event the store holds (page rows,
 * page aux, live overlay, live aux), returning the same store reference when
 * nothing changed.
 *
 * Local writes MUST go through this rather than patching the flattened
 * `channelMessagesKey` array alone: the window store is the source of truth,
 * and every live merge re-flattens it into `channelMessagesKey`
 * (`flattenChannelWindowEvents`), so an update applied only to the flattened
 * array is silently reverted by the next live event. The message-edit
 * apply-on-success update hit exactly that: the edit rendered, then the next
 * live merge re-flattened the un-edited store over it. (Masked on busy
 * screens because unrelated re-renders kept re-deriving state; exposed once
 * per-keystroke app-shell renders were removed.)
 */
export function mapChannelWindowEvents(
  store: ChannelWindowStore,
  map: (event: RelayEvent) => RelayEvent,
): ChannelWindowStore {
  let changed = false;
  const mapEvent = (event: RelayEvent) => {
    const next = map(event);
    if (next !== event) changed = true;
    return next;
  };
  const pages = store.pages.map((page) => {
    const rows = page.rows.map((row) => {
      const event = mapEvent(row.event);
      return event === row.event ? row : { ...row, event };
    });
    const aux = page.aux.map(mapEvent);
    return rows.every((row, index) => row === page.rows[index]) &&
      aux.every((event, index) => event === page.aux[index])
      ? page
      : { ...page, rows, aux };
  });
  const liveOverlay = store.liveOverlay.map(mapEvent);
  const liveAux = store.liveAux.map(mapEvent);
  return changed ? { ...store, pages, liveOverlay, liveAux } : store;
}

/** Raw events in the chronological order expected by the existing renderer. */
export function flattenChannelWindowEvents(store: ChannelWindowStore) {
  const byId = new Map<string, RelayEvent>();
  for (const page of store.pages) {
    for (const row of page.rows) byId.set(row.event.id, row.event);
    for (const event of page.aux) byId.set(event.id, event);
  }
  for (const event of store.liveOverlay) byId.set(event.id, event);
  for (const event of store.liveAux) byId.set(event.id, event);
  return [...byId.values()].sort((left, right) =>
    compareRelayOrder(right, left),
  );
}

/**
 * Whether older history remains beyond the retained pages. The authoritative
 * signal for paging: the tail page's `hasMore` (kept in lockstep with its
 * `nextCursor` by `assertValidPage`). Empty pages mean no window has loaded
 * yet — there is no cursor to page with, so report false; the first
 * `replaceNewestChannelWindow` makes the signal authoritative.
 */
export function channelWindowHasMore(store: ChannelWindowStore) {
  const tail = store.pages[store.pages.length - 1];
  return tail?.hasMore ?? false;
}

/**
 * Whether the loaded window PROVABLY starts at the channel's beginning. This
 * is not `!channelWindowHasMore`: an empty store also reports "no more", but
 * that means the boundary is unresolved (nothing has loaded), not exhausted.
 * Only a resolved tail page saying `hasMore: false` proves the start.
 */
export function channelWindowHistoryExhausted(store: ChannelWindowStore) {
  const tail = store.pages[store.pages.length - 1];
  return tail !== undefined && !tail.hasMore;
}

/**
 * Per-root thread summaries for badge rendering: authoritative page summaries
 * overlaid with any fresher relay-pushed live summaries. The live overlay also
 * covers roots that reached the screen outside a page (liveOverlay rows,
 * refetch-reconciled rows), which have no page summary at all.
 */
export function channelWindowThreadSummaries(store: ChannelWindowStore) {
  const summaries = new Map(
    store.pages.flatMap((page) =>
      page.rows.flatMap((row) =>
        row.thread ? ([[row.event.id, row.thread]] as const) : [],
      ),
    ),
  );
  for (const [rootId, live] of Object.entries(store.liveSummaries)) {
    summaries.set(rootId, live.summary);
  }
  return summaries;
}
