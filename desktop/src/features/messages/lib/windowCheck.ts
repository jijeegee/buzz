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
import {
  liveActivityKey,
  liveRoomActivity,
} from "@/shared/api/liveRoomActivity";
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

/**
 * A check's reply: the check; or `noCheck` when the relay answered without
 * one (a relay without the check), with its room page. Neither for an error
 * or an empty answer: they say nothing about the relay.
 */
export type WindowCheckReply = {
  answer: WindowCheckAnswer | null;
  noCheck: boolean;
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
  /**
   * Draws a stored window below a cursor at once (scroll-up), before the
   * check answers; the open's answer then replaces it.
   */
  paint?: (stored: T) => void;
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
  no_check?: boolean;
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
  /** Whether the page was applied. */
  replace: (
    mark: string,
    scope: StoreScope,
    key: WindowKey,
    oldest: WindowCursor | null,
    ids: string[],
  ) => Promise<boolean>;
  /** Sends `request`, then records the head spans `answers` reads off it. */
  withHeads: <R>(
    roomIds: readonly string[],
    request: () => Promise<R>,
    answers: (answer: R) => StoreHeadAnswer[],
  ) => Promise<R>;
  /** Live events so far for a room, or a room's thread. */
  liveActivity: (key: WindowKey) => number;
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
      noCheck: raw?.no_check === true,
      page: raw?.page ?? null,
    };
  },
  mark: (roomId) =>
    invokeTauri<string | null>("message_store_head_sent", {
      roomIds: [roomId],
    }),
  release: (mark) =>
    invokeTauri<void>("message_store_record_heads", { scope: mark, heads: [] }),
  replace: (mark, scope, key, oldest, ids) =>
    invokeTauri<boolean>("message_store_replace_window", {
      scope: mark,
      account: scope.pubkey,
      relayUrl: scope.relayUrl,
      channelId: key.roomId,
      rootId: key.rootId,
      cursor: wireCursor(key.cursor),
      oldest: wireCursor(oldest),
      ids,
    }),
  withHeads: requestWithStoreHeads,
  liveActivity: (key) =>
    liveRoomActivity(liveActivityKey(key.roomId, key.rootId)),
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
function isOlder(row: RelayEvent, than: RelayEvent) {
  return (
    row.created_at < than.created_at ||
    (row.created_at === than.created_at && row.id > than.id)
  );
}

function oldestRow(rows: readonly RelayEvent[]): RelayEvent | null {
  let oldest: RelayEvent | null = null;
  for (const row of rows) if (!oldest || isOlder(row, oldest)) oldest = row;
  return oldest;
}

function newestRow(rows: readonly RelayEvent[]): RelayEvent | null {
  let newest: RelayEvent | null = null;
  for (const row of rows) if (!newest || isOlder(newest, row)) newest = row;
  return newest;
}

/**
 * The hex id just before `id` (same length), so a cursor `(at, before)`
 * takes in `id` itself; null when there is none.
 */
export function hexIdBefore(id: string): string | null {
  const lower = id.toLowerCase();
  if (!/^[0-9a-f]+$/.test(lower)) return null;
  const digits = [...lower];
  for (let index = digits.length - 1; index >= 0; index -= 1) {
    const value = Number.parseInt(digits[index], 16);
    if (value > 0) {
      digits[index] = (value - 1).toString(16);
      return digits.join("");
    }
    digits[index] = "f";
  }
  return null;
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

const NO_REPLY: WindowCheckReply = { answer: null, noCheck: false, page: null };

/** A check that never fails: an error says nothing about the relay. */
function checkQuietly(key: WindowKey): Promise<WindowCheckReply> {
  return deps
    .check(key)
    .then((reply) => reply ?? NO_REPLY)
    .catch(() => NO_REPLY);
}

/**
 * Learns the relay's mode from a real answer only: its check, or `off` for
 * an answer without one. An error or empty answer leaves it as it was.
 */
function noteMode(scope: StoreScope, reply: WindowCheckReply) {
  if (reply.answer) {
    noteServerWindowCheckMode(scope.relayUrl, reply.answer.mode, deps.nowMs());
  } else if (reply.noCheck) {
    noteServerWindowCheckMode(scope.relayUrl, "off", deps.nowMs());
  }
}

function release(mark: string | null) {
  if (mark !== null) void deps.release(mark).catch(() => {});
}

/**
 * After a refetch was applied, a store still different from the page (rows
 * at or below the page's newest; later live rows do not count) guards the
 * window. A store that cannot be read is not known to differ.
 */
async function guardIfStillDifferent(
  scope: StoreScope,
  key: WindowKey,
  page: PageRows,
) {
  const newest = newestRow(page.rows);
  let below = key;
  if (newest) {
    const before = hexIdBefore(newest.id);
    if (before === null) return;
    below = {
      ...key,
      cursor: { createdAt: newest.created_at, eventId: before },
    };
  }
  const after = await readStored(scope, below);
  if (!after || same(ends(after), pageEnds(page))) return;
  guarded.set(guardKey(key), deps.nowMs() + WINDOW_CHECK_GUARD_MS);
  deps.log(
    `[window-check] ${label(key)} still differs after a refetch; today's path for 10 min`,
  );
}

/**
 * Fetches the page with its overlays and keeps it as the window's stretch in
 * the store. With `guard` (a refetch after a mismatch), a store that still
 * differs from the applied page puts the room or thread on today's path for
 * a while; that is looked at behind the answer, never before it.
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
  const kept = page;
  const oldest = kept.hasMore ? oldestRow(kept.rows) : null;
  void deps
    .replace(
      mark,
      scope,
      key,
      oldest ? { createdAt: oldest.created_at, eventId: oldest.id } : null,
      kept.ids,
    )
    .then((applied) => {
      if (applied && guard) return guardIfStillDifferent(scope, key, kept);
    })
    .catch(() => {});
  return answer;
}

/** A check sent behind today's request, only to learn the relay's mode. */
function probe(scope: StoreScope, key: WindowKey) {
  void checkQuietly(key).then((reply) => noteMode(scope, reply));
}

/** What a check (and its one re-check) came to. */
type Checked =
  | { kind: "fetch" }
  | { kind: "equal"; stored: StoredWindow }
  | {
      kind: "differ" | "shadowEqual";
      stored: StoredWindow;
      mode: WindowCheckMode;
    }
  | { kind: "off"; page: RelayEvent[] | null };

/**
 * Mode `on`: the check, and alongside it the store mark and stored read.
 * A stored window below a cursor is drawn as soon as it is read. Equal: the
 * stored window is kept as the page it stands for (page span).
 */
async function checkStored<T>(
  scope: StoreScope,
  key: WindowKey,
  io: WindowIo<T>,
): Promise<Checked> {
  // The check goes out first; the mark (in memory, no store wait) is taken
  // before the store read, so rows written after the read are never dropped.
  let live = deps.liveActivity(key);
  const checking = checkQuietly(key);
  const mark = await deps.mark(key.roomId).catch(() => null);
  let stored = await readStored(scope, key);
  if (!stored?.fullyStored) {
    release(mark);
    void checking.then((reply) => noteMode(scope, reply));
    return { kind: "fetch" };
  }
  if (key.cursor && io.paint) io.paint(io.fromStore(stored));
  let reply = await checking;
  noteMode(scope, reply);
  let answer = reply.answer;
  // No answer: this window only takes today's path.
  const mode = answer ? effectiveWindowCheckMode(answer.mode) : "off";
  if (!answer || mode === "off") {
    release(mark);
    return { kind: "off", page: reply.noCheck ? reply.page : null };
  }
  let equal = same(ends(stored), answer);
  if (!equal && deps.liveActivity(key) !== live) {
    // Live rows arrived while the check was out: compare once more.
    live = deps.liveActivity(key);
    const [again, second] = await Promise.all([
      readStored(scope, key),
      checkQuietly(key),
    ]);
    if (again?.fullyStored && second.answer) {
      stored = again;
      reply = second;
      answer = second.answer;
      equal = same(ends(stored), answer);
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
        scope,
        key,
        stored.hasMore ? stored.oldest : null,
        stored.events.map((event) => event.id),
      )
      .catch(() => false);
  }
  return { kind: "equal", stored };
}

/**
 * Shadow: today's page (kept) goes out at once, the stored read and the
 * check alongside it; the comparison is only logged. A stored window below
 * a cursor is drawn when it is read before the page.
 */
async function shadowWindow<T>(
  scope: StoreScope,
  key: WindowKey,
  io: WindowIo<T>,
): Promise<T> {
  const started = deps.nowMs();
  let answered = false;
  const checking = checkQuietly(key).then((reply) => {
    noteMode(scope, reply);
    return { reply, ms: deps.nowMs() - started };
  });
  const reading = readStored(scope, key).then((stored) => {
    if (stored?.fullyStored && key.cursor && io.paint && !answered) {
      io.paint(io.fromStore(stored));
    }
    return stored;
  });
  const result = await fetchAndKeep(scope, key, io, true);
  answered = true;
  const pageMs = deps.nowMs() - started;
  void Promise.all([checking, reading]).then(([{ reply, ms }, stored]) => {
    if (!reply.answer || !stored?.fullyStored) return;
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
  if (mode === "probe") {
    // Not known yet: today's request at once, the check behind it.
    probe(scope, key);
    return io.fetch(false);
  }
  if (mode === "shadow") return shadowWindow(scope, key, io);

  // `on`: an equal answer, or an old relay's page, records the head span
  // today's request would have.
  const started = deps.nowMs();
  const head = io.head;
  const run = () => checkStored(scope, key, io);
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
  if (checked.kind === "fetch") return fetchAndKeep(scope, key, io, false);
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
