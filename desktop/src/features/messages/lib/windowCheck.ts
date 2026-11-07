/**
 * One open path for room and thread windows, newest or below a cursor
 * (docs/message-sync.md#window-check):
 *
 * 1. The window is drawn from the store first, as before: a newest window
 *    paints its stored rows while this path runs (storeRoomHead.ts,
 *    threadWindowCache.ts), so no mode waits on the check to draw.
 * 2. A window the store does not hold (no relay page or head span reaches
 *    it) is fetched as today and kept as that window's page.
 * 3. Otherwise the relay checks it: its oldest row id and row count, no rows.
 * 4. Equal: mode `on` answers with the stored window and records what its
 *    page would have (page span; head span for a newest window); `shadow`
 *    fetches alongside the check and logs.
 * 5. Different: the page is fetched and replaces the stored rows of its
 *    stretch, and the open redraws with it. Still different right after:
 *    that room or thread takes today's path for 10 minutes.
 *
 * Mode `off` (the relay's, or the developer setting) is today's path. A
 * relay without the check counts as `off`; one that answers a room check
 * with the page itself has that page used.
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
import {
  requestWithStoreHeads,
  type StoreHeadAnswer,
} from "@/shared/api/relayStoreHeads";
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
  /** A room: a channel-window response; a thread: its replies and overlays. */
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

/** A check's reply: the check, or an old relay's room page. */
export type WindowCheckReply = {
  answer: WindowCheckAnswer | null;
  page: RelayEvent[] | null;
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
  /** An old relay's answer to a room check, as the page (rooms only). */
  fromCheckPage?: (events: RelayEvent[]) => T;
  /**
   * The head span an answer vouches for, when the request records one (a
   * newest window asked with `storeHead`), as today's request would.
   */
  head?: (answer: T) => StoreHeadAnswer | null;
};

type RawStoredWindow = {
  events: RelayEvent[];
  count: number;
  oldest_id: string | null;
  oldest_created_at: number | null;
  has_more: boolean;
  fully_stored: boolean;
};

type RawCheckReply = {
  check: {
    oldest_id: string | null;
    count: number;
    mode: WindowCheckMode;
  } | null;
  page: RelayEvent[] | null;
};

function wireCursor(cursor: WindowCursor | null) {
  return cursor
    ? { created_at: cursor.createdAt, event_id: cursor.eventId }
    : null;
}

type WindowCheckDeps = {
  stored: (scope: StoreScope, key: WindowKey) => Promise<StoredWindow | null>;
  check: (key: WindowKey) => Promise<WindowCheckReply>;
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
  /** Sends `request`, then records the head spans `answers` reads off it. */
  withHeads: <R>(
    roomIds: readonly string[],
    request: () => Promise<R>,
    answers: (answer: R) => StoreHeadAnswer[],
  ) => Promise<R>;
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
    const raw = await invokeTauri<RawCheckReply>("check_window", {
      channelId: key.roomId,
      rootId: key.rootId,
      cursor: wireCursor(key.cursor),
      limitRows: key.limit,
    });
    return {
      answer: raw?.check
        ? {
            oldestId: raw.check.oldest_id,
            count: raw.check.count,
            mode: raw.check.mode,
          }
        : null,
      page: raw?.page ?? null,
    };
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
  withHeads: requestWithStoreHeads,
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

type Ends = { count: number; oldestId: string | null };

function same(stored: Ends, other: Ends) {
  return (
    stored.count === other.count &&
    (stored.oldestId ?? null)?.toLowerCase() ===
      (other.oldestId ?? null)?.toLowerCase()
  );
}

const ends = (stored: StoredWindow): Ends => ({
  count: stored.count,
  oldestId: stored.oldest?.eventId ?? null,
});

function pageEnds(page: PageRows): Ends {
  return {
    count: page.rows.length,
    oldestId: oldestRow(page.rows)?.id ?? null,
  };
}

/** Reads the stored window; a failed read counts as nothing stored. */
function readStored(scope: StoreScope, key: WindowKey) {
  return deps.stored(scope, key).catch(() => null);
}

/** A check that never fails: an error is a relay without the check. */
function checkQuietly(key: WindowKey): Promise<WindowCheckReply> {
  return deps
    .check(key)
    .then((reply) => reply ?? { answer: null, page: null })
    .catch(() => ({ answer: null, page: null }));
}

function release(mark: string | null) {
  if (mark !== null) void deps.release(mark).catch(() => {});
}

/** After a refetch, a store still different from the page guards the window. */
async function guardIfStillDifferent(
  scope: StoreScope,
  key: WindowKey,
  page: PageRows,
) {
  const after = await readStored(scope, key);
  if (after && same(ends(after), pageEnds(page))) return;
  guarded.set(guardKey(key), deps.nowMs() + WINDOW_CHECK_GUARD_MS);
  deps.log(
    `[window-check] ${label(key)} still differs after a refetch; today's path for 10 min`,
  );
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
    release(mark);
    throw error;
  }
  if (mark === null) return answer;
  if (!page) {
    release(mark);
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
  if (guard) await guardIfStillDifferent(scope, key, page);
  return answer;
}

/** A check whose answer only teaches the relay's mode. */
function probe(scope: StoreScope, key: WindowKey) {
  void checkQuietly(key).then((reply) =>
    noteServerWindowCheckMode(
      scope.relayUrl,
      reply.answer?.mode ?? "off",
      deps.nowMs(),
    ),
  );
}

/** What a check (and its one re-check) came to. */
type Checked =
  | { kind: "equal"; stored: StoredWindow }
  | {
      kind: "differ" | "shadowEqual";
      stored: StoredWindow;
      mode: WindowCheckMode;
    }
  | { kind: "off"; page: RelayEvent[] | null };

/**
 * Checks `stored` against the relay. Equal in mode `on`: the stored window
 * is kept as the page it stands for (page span), under a mark taken before
 * the check went out.
 */
async function checkStored(
  scope: StoreScope,
  key: WindowKey,
  first: StoredWindow,
): Promise<Checked> {
  const mark = await deps.mark(key.roomId).catch(() => null);
  let stored = first;
  let live = deps.liveActivity(key.roomId);
  let reply = await checkQuietly(key);
  noteServerWindowCheckMode(
    scope.relayUrl,
    reply.answer?.mode ?? "off",
    deps.nowMs(),
  );
  let answer = reply.answer;
  const mode = effectiveWindowCheckMode(answer?.mode ?? "off");
  if (!answer || mode === "off") {
    release(mark);
    return { kind: "off", page: answer ? null : reply.page };
  }
  let equal = same(ends(stored), answer);
  if (!equal && deps.liveActivity(key.roomId) !== live) {
    // Live rows arrived while the check was out: compare once more.
    live = deps.liveActivity(key.roomId);
    const again = await readStored(scope, key);
    if (again?.fullyStored) {
      reply = await checkQuietly(key);
      if (reply.answer) {
        stored = again;
        answer = reply.answer;
        equal = same(ends(stored), answer);
      }
    }
  }
  if (!equal || mode !== "on") {
    release(mark);
    return { kind: equal ? "shadowEqual" : "differ", stored, mode };
  }
  if (mark !== null) {
    await deps
      .replace(
        mark,
        key,
        stored.hasMore ? stored.oldest : null,
        stored.events.map((event) => event.id),
      )
      .catch(() => {});
  }
  return { kind: "equal", stored };
}

/**
 * Shadow: the page is fetched (and kept) alongside the check, so the open is
 * no later than today's; the check is only logged.
 */
async function shadowWindow<T>(
  scope: StoreScope,
  key: WindowKey,
  io: WindowIo<T>,
  stored: StoredWindow,
): Promise<T> {
  const started = deps.nowMs();
  const checked = checkQuietly(key).then((reply) => {
    noteServerWindowCheckMode(
      scope.relayUrl,
      reply.answer?.mode ?? "off",
      deps.nowMs(),
    );
    return { reply, ms: deps.nowMs() - started };
  });
  const result = await fetchAndKeep(scope, key, io, true);
  const pageMs = deps.nowMs() - started;
  void checked.then(({ reply, ms }) => {
    if (!reply.answer) return;
    const fetched = io.page(result);
    const equal = same(ends(stored), reply.answer);
    const agrees = fetched !== null && same(ends(stored), pageEnds(fetched));
    deps.log(
      `[window-check] shadow ${label(key)} ${equal ? "equal" : "differ"}, page ${agrees ? "agrees" : "differs"} (check ${ms} ms, page ${pageMs} ms)`,
    );
  });
  return result;
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

  const stored = await readStored(scope, key);
  if (!stored?.fullyStored) {
    if (mode === "probe") {
      probe(scope, key);
      return io.fetch(false);
    }
    return fetchAndKeep(scope, key, io, false);
  }
  if (mode === "shadow") return shadowWindow(scope, key, io, stored);

  // `on`, or the mode is not known yet: the check goes first. Its answer,
  // when equal or an old relay's page, records the head span today's
  // request would have.
  const started = deps.nowMs();
  const head = io.head;
  const run = () => checkStored(scope, key, stored);
  const checked = head
    ? await deps.withHeads([key.roomId], run, (out) => {
        const answer =
          out.kind === "equal"
            ? head(io.fromStore(out.stored))
            : out.kind === "off" && out.page && io.fromCheckPage
              ? head(io.fromCheckPage(out.page))
              : null;
        return answer ? [answer] : [];
      })
    : await run();
  const checkMs = deps.nowMs() - started;
  if (checked.kind === "off") {
    return checked.page && io.fromCheckPage
      ? io.fromCheckPage(checked.page)
      : io.fetch(false);
  }
  if (checked.kind === "equal") {
    deps.log(`[window-check] on ${label(key)} equal, no fetch (${checkMs} ms)`);
    return io.fromStore(checked.stored);
  }
  if (checked.kind === "shadowEqual") {
    // The relay said shadow on this first check: today's page, logged.
    const result = await io.fetch(false);
    deps.log(`[window-check] shadow ${label(key)} equal (check ${checkMs} ms)`);
    return result;
  }
  const fetchedAt = deps.nowMs();
  const result = await fetchAndKeep(scope, key, io, true);
  deps.log(
    `[window-check] ${checked.mode} ${label(key)} differ, refetched (check ${checkMs} ms, page ${deps.nowMs() - fetchedAt} ms)`,
  );
  return result;
}
