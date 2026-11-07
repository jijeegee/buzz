/**
 * One open path for room and thread windows, newest or below a cursor
 * (docs/message-sync.md#window-check):
 *
 * 1. The window is drawn from the store as today (paint, or a stored page).
 * 2. A window the store does not hold (no relay page or head span reaches
 *    it) is fetched as today and kept as that window's page.
 * 3. Otherwise the relay checks it: its oldest row id and row count, no rows.
 * 4. Equal: mode `on` uses the stored window (no page); `shadow` still
 *    fetches and logs.
 * 5. Different: the page is fetched and replaces the stored rows of its
 *    stretch. Still different right after: that room or thread takes
 *    today's path for 10 minutes.
 *
 * Mode `off` (the relay's, or the developer setting) is today's path.
 */
import type { QueryClient } from "@tanstack/react-query";

import { type StoreScope, storeScope } from "./paintCoverage";
import {
  effectiveWindowCheckMode,
  noteServerWindowCheckMode,
  type WindowCheckMode,
  windowCheckModeFor,
} from "./windowCheckMode";
import { liveRoomActivity } from "@/shared/api/liveRoomActivity";
import { invokeTauri } from "@/shared/api/tauri";
import type { RelayEvent } from "@/shared/api/types";

export type WindowCursor = { createdAt: number; eventId: string };

/** One window: a room's (`rootId` null) or a thread's, below `cursor`. */
export type WindowKey = {
  roomId: string;
  rootId: string | null;
  cursor: WindowCursor | null;
  limit: number;
};

/** A window as the store holds it. */
export type StoredWindow = {
  /** A room: a channel-window response; a thread: its replies. */
  events: RelayEvent[];
  count: number;
  oldest: WindowCursor | null;
  hasMore: boolean;
  /** A relay page or head span reaches it: worth checking. */
  fullyStored: boolean;
};

export type WindowCheckAnswer = {
  oldestId: string | null;
  count: number;
  mode: WindowCheckMode;
};

/** What a fetched page holds. */
export type PageRows = {
  /** Every event id of the page (rows and overlays). */
  ids: string[];
  /** The window's rows (the kinds and rule the check counts). */
  rows: readonly RelayEvent[];
  hasMore: boolean;
};

/** How one caller fetches its page and reads it, for {@link checkedWindow}. */
export type WindowIo<T> = {
  /** Today's page request; `includeAux` asks a thread page for overlays. */
  fetch: (includeAux: boolean) => Promise<T>;
  /** What the page holds; null when it is no window (no access). */
  page: (answer: T) => PageRows | null;
  fromStore: (stored: StoredWindow) => T;
};

type RawStoredWindow = {
  events: RelayEvent[];
  count: number;
  oldest_id: string | null;
  oldest_created_at: number | null;
  has_more: boolean;
  fully_stored: boolean;
};

type RawCheckAnswer = {
  oldest_id: string | null;
  count: number;
  mode: WindowCheckMode;
};

function wireCursor(cursor: WindowCursor | null) {
  return cursor
    ? { created_at: cursor.createdAt, event_id: cursor.eventId }
    : null;
}

type WindowCheckDeps = {
  stored: (scope: StoreScope, key: WindowKey) => Promise<StoredWindow | null>;
  check: (key: WindowKey) => Promise<WindowCheckAnswer | null>;
  /** Marks a page request going out (the store's capture), or null. */
  mark: (roomId: string) => Promise<string | null>;
  /** Releases a mark no page was kept for. */
  release: (mark: string) => Promise<void>;
  replace: (
    mark: string,
    key: WindowKey,
    oldest: WindowCursor | null,
    ids: string[],
  ) => Promise<void>;
  liveActivity: (roomId: string) => number;
  nowMs: () => number;
  log: (line: string) => void;
};

const defaultDeps: WindowCheckDeps = {
  stored: async (scope, key) => {
    const raw = await invokeTauri<RawStoredWindow | null>(
      "message_store_window",
      {
        channelId: key.roomId,
        rootId: key.rootId,
        cursor: wireCursor(key.cursor),
        limit: key.limit,
        account: scope.pubkey,
        relayUrl: scope.relayUrl,
      },
    );
    if (!raw) return null;
    return {
      events: raw.events,
      count: raw.count,
      oldest:
        raw.oldest_id !== null && raw.oldest_created_at !== null
          ? { createdAt: raw.oldest_created_at, eventId: raw.oldest_id }
          : null,
      hasMore: raw.has_more,
      fullyStored: raw.fully_stored,
    };
  },
  check: async (key) => {
    const raw = await invokeTauri<RawCheckAnswer | null>("check_window", {
      channelId: key.roomId,
      rootId: key.rootId,
      cursor: wireCursor(key.cursor),
      limitRows: key.limit,
    });
    return raw
      ? { oldestId: raw.oldest_id, count: raw.count, mode: raw.mode }
      : null;
  },
  mark: (roomId) =>
    invokeTauri<string | null>("message_store_head_sent", {
      roomIds: [roomId],
    }),
  release: (mark) =>
    invokeTauri<void>("message_store_record_heads", { scope: mark, heads: [] }),
  replace: (mark, key, oldest, ids) =>
    invokeTauri<void>("message_store_replace_window", {
      scope: mark,
      channelId: key.roomId,
      rootId: key.rootId,
      cursor: wireCursor(key.cursor),
      oldest: wireCursor(oldest),
      ids,
    }),
  liveActivity: liveRoomActivity,
  nowMs: () => Date.now(),
  log: (line) => console.info(line),
};

let deps = defaultDeps;

/** Replace the store and relay bridge (tests). Returns the restore function. */
export function setWindowCheckDeps(next: Partial<WindowCheckDeps>) {
  const previous = deps;
  deps = { ...deps, ...next };
  return () => {
    deps = previous;
  };
}

/** A room or thread still different right after a refetch: until when. */
export const WINDOW_CHECK_GUARD_MS = 10 * 60_000;
const guarded = new Map<string, number>();

function guardKey(key: WindowKey) {
  return key.rootId ? `${key.roomId}/${key.rootId}` : key.roomId;
}

function isGuarded(key: WindowKey) {
  const until = guarded.get(guardKey(key));
  if (until === undefined) return false;
  if (deps.nowMs() < until) return true;
  guarded.delete(guardKey(key));
  return false;
}

/** Forget loop guards (tests). */
export function resetWindowCheckGuards() {
  guarded.clear();
}

function label(key: WindowKey) {
  const where = key.rootId
    ? `thread ${key.rootId.slice(0, 8)}`
    : `room ${key.roomId.slice(0, 8)}`;
  return key.cursor
    ? `${where} below ${key.cursor.eventId.slice(0, 8)}`
    : where;
}

/** Relay order: newest first, then ascending id within a second. */
function oldestRow(rows: readonly RelayEvent[]): RelayEvent | null {
  let oldest: RelayEvent | null = null;
  for (const row of rows) {
    if (
      !oldest ||
      row.created_at < oldest.created_at ||
      (row.created_at === oldest.created_at && row.id > oldest.id)
    ) {
      oldest = row;
    }
  }
  return oldest;
}

function same(
  stored: { count: number; oldestId: string | null },
  other: { count: number; oldestId: string | null },
) {
  return (
    stored.count === other.count &&
    (stored.oldestId ?? null)?.toLowerCase() ===
      (other.oldestId ?? null)?.toLowerCase()
  );
}

const ends = (stored: StoredWindow) => ({
  count: stored.count,
  oldestId: stored.oldest?.eventId ?? null,
});

function pageEnds(page: PageRows) {
  return {
    count: page.rows.length,
    oldestId: oldestRow(page.rows)?.id ?? null,
  };
}

/** Reads the stored window; a failed read counts as nothing stored. */
function readStored(scope: StoreScope, key: WindowKey) {
  return deps.stored(scope, key).catch(() => null);
}

/**
 * Fetches the page with its overlays and keeps it as the window's stretch in
 * the store. With `guard` (a refetch after a mismatch), a store that still
 * differs from the page puts the room or thread on today's path for a while.
 */
async function fetchAndKeep<T>(
  scope: StoreScope,
  key: WindowKey,
  io: WindowIo<T>,
  guard: boolean,
): Promise<T> {
  const mark = await deps.mark(key.roomId).catch(() => null);
  let answer: T;
  let page: PageRows | null;
  try {
    answer = await io.fetch(true);
    page = mark === null ? null : io.page(answer);
  } catch (error) {
    if (mark !== null) void deps.release(mark).catch(() => {});
    throw error;
  }
  if (mark === null) return answer;
  if (!page) {
    void deps.release(mark).catch(() => {});
    return answer;
  }
  const oldest = page.hasMore ? oldestRow(page.rows) : null;
  try {
    await deps.replace(
      mark,
      key,
      oldest ? { createdAt: oldest.created_at, eventId: oldest.id } : null,
      page.ids,
    );
  } catch {
    return answer;
  }
  if (!guard) return answer;
  const after = await readStored(scope, key);
  if (!after || !same(ends(after), pageEnds(page))) {
    guarded.set(guardKey(key), deps.nowMs() + WINDOW_CHECK_GUARD_MS);
    deps.log(
      `[window-check] ${label(key)} still differs after a refetch; today's path for 10 min`,
    );
  }
  return answer;
}

/** A check whose answer only teaches the relay's mode. */
function probe(scope: StoreScope, key: WindowKey) {
  void deps
    .check(key)
    .then((answer) =>
      noteServerWindowCheckMode(
        scope.relayUrl,
        answer?.mode ?? "off",
        deps.nowMs(),
      ),
    )
    .catch(() => {});
}

/**
 * The window `key` for `queryClient`'s account and server, by the one open
 * path above. `io.fetch(false)` is exactly today's request.
 */
export async function checkedWindow<T>(
  queryClient: QueryClient,
  key: WindowKey,
  io: WindowIo<T>,
): Promise<T> {
  const scope = storeScope(queryClient);
  if (!scope) return io.fetch(false);
  const mode = windowCheckModeFor(scope.relayUrl, deps.nowMs());
  if (mode === "off" || isGuarded(key)) return io.fetch(false);

  let stored = await readStored(scope, key);
  if (!stored?.fullyStored) {
    if (mode === "probe") {
      probe(scope, key);
      return io.fetch(false);
    }
    return fetchAndKeep(scope, key, io, false);
  }

  const started = deps.nowMs();
  let live = deps.liveActivity(key.roomId);
  let answer = await deps.check(key).catch(() => null);
  noteServerWindowCheckMode(
    scope.relayUrl,
    answer?.mode ?? "off",
    deps.nowMs(),
  );
  const effective = effectiveWindowCheckMode(answer?.mode ?? "off");
  if (!answer || effective === "off") return io.fetch(false);
  let equal = same(ends(stored), answer);
  if (!equal && deps.liveActivity(key.roomId) !== live) {
    // Live rows arrived while the check was out: compare once more.
    live = deps.liveActivity(key.roomId);
    const again = await readStored(scope, key);
    const second = again?.fullyStored
      ? await deps.check(key).catch(() => null)
      : null;
    if (again && second) {
      stored = again;
      answer = second;
      equal = same(ends(stored), answer);
    }
  }
  const checkMs = deps.nowMs() - started;
  if (equal && effective === "on") {
    deps.log(`[window-check] on ${label(key)} equal, no fetch (${checkMs} ms)`);
    return io.fromStore(stored);
  }
  const fetchedAt = deps.nowMs();
  const result = equal
    ? await io.fetch(false)
    : await fetchAndKeep(scope, key, io, true);
  const pageMs = deps.nowMs() - fetchedAt;
  const fetched = io.page(result);
  // In shadow, whether skipping the page would have been right.
  const verdict = equal
    ? fetched && same(ends(stored), pageEnds(fetched))
      ? "equal, page agrees"
      : "equal, page differs"
    : "differ, refetched";
  deps.log(
    `[window-check] ${effective} ${label(key)} ${verdict} (check ${checkMs} ms, page ${pageMs} ms)`,
  );
  return result;
}
