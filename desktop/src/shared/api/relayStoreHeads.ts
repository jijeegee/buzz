/**
 * Head spans of the device message store: a newest window the relay
 * answered while a live REQ carrying its room kept receiving vouches for
 * every row of that window from its oldest row on
 * (docs/message-sync.md#head-spans). Recorded only when every such REQ live
 * as the request went out is still live, unchanged, when the answer
 * arrives. Only the newest window a room or thread opens on, and the
 * preload, opt in; and only at the open window's size or wider, so a
 * narrower answer never stands in for the open window.
 */
import {
  type StoreLiveCoverage,
  storeLiveCoverage,
} from "@/shared/api/relayStoreCoverage";
import { invokeTauri } from "@/shared/api/tauri";
import type { RelayEvent } from "@/shared/api/types";

/** Rows of the window a room or thread opens on; narrower answers record nothing. */
export const STORE_HEAD_MIN_ROWS = 50;

/** Whether a newest-window request opted in and is wide enough to record. */
export function recordsStoreHead(
  options: { storeHead?: boolean },
  limitRows: number,
) {
  return options.storeHead === true && limitRows >= STORE_HEAD_MIN_ROWS;
}

/** Kind of the bounds overlay that closes every channel window. */
const WINDOW_BOUNDS_KIND = 39006;

type Cursor = { createdAt: number; eventId: string };

/** One newest window as the relay answered it. */
export type StoreHeadAnswer = {
  roomId: string;
  /** `null`: the room's window; else the window of this thread root. */
  rootId: string | null;
  hasMore: boolean;
  /** The answer's oldest row, when `hasMore`. */
  nextCursor: Cursor | null;
  events: readonly RelayEvent[];
};

type StoreHeadSpan = {
  roomId: string;
  rootId: string | null;
  liveAt: number;
  nextCursor: Cursor | null;
  ids: string[];
};

type StoreHeadDeps = {
  /** Marks the request going out; its capture, or null (nothing recorded). */
  sent: (roomIds: string[]) => Promise<string | null>;
  /** Records `heads` for the request marked `scope`; empty only releases it. */
  record: (scope: string, heads: StoreHeadSpan[]) => Promise<void>;
};

const defaultDeps: StoreHeadDeps = {
  sent: (roomIds) =>
    invokeTauri<string | null>("message_store_head_sent", { roomIds }),
  record: (scope, heads) =>
    invokeTauri<void>("message_store_record_heads", { scope, heads }),
};

let deps = defaultDeps;

/** Replace the store bridge (tests). Returns the restore function. */
export function setStoreHeadDeps(next: Partial<StoreHeadDeps>) {
  const previous = deps;
  deps = { ...deps, ...next };
  return () => {
    deps = previous;
  };
}

/** The live REQs carrying a room as a request goes out, and their L. */
type LiveToken = { coverage: StoreLiveCoverage[]; liveAt: number };

function liveToken(roomId: string): LiveToken | null {
  const coverage = storeLiveCoverage(roomId);
  if (coverage.length === 0) return null;
  const liveAt = Math.max(...coverage.map((live) => live.coverage.liveAt));
  return { coverage, liveAt };
}

/** Each REQ of the token has been live without a gap since it was taken. */
function holds(token: LiveToken) {
  return token.coverage.every(
    ({ subscription, coverage }) => subscription.storeCoverage === coverage,
  );
}

function headsOf(
  answers: readonly StoreHeadAnswer[],
  tokens: ReadonlyMap<string, LiveToken>,
): StoreHeadSpan[] {
  const heads: StoreHeadSpan[] = [];
  for (const answer of answers) {
    const token = tokens.get(answer.roomId);
    if (!token || !holds(token)) continue;
    if (answer.hasMore && !answer.nextCursor) continue;
    heads.push({
      roomId: answer.roomId,
      rootId: answer.rootId,
      liveAt: token.liveAt,
      nextCursor: answer.hasMore ? answer.nextCursor : null,
      ids: answer.events.map((event) => event.id),
    });
  }
  return heads;
}

/**
 * Send newest-window `request` for `roomIds`, then record the head span of
 * each window `answers` reads out of its answer whose room was live when the
 * request went out and still is. The request is marked in the store first
 * (awaited), so rows written after it are never taken for deleted ones.
 */
export async function requestWithStoreHeads<T>(
  roomIds: readonly string[],
  request: () => Promise<T>,
  answers: (answer: T) => StoreHeadAnswer[],
): Promise<T> {
  const tokens = new Map<string, LiveToken>();
  for (const roomId of roomIds) {
    const token = liveToken(roomId);
    if (token) tokens.set(roomId, token);
  }
  if (tokens.size === 0) return request();
  const scope = await deps.sent([...tokens.keys()]).catch(() => null);
  if (scope === null) return request();
  let heads: StoreHeadSpan[] = [];
  try {
    const answer = await request();
    try {
      heads = headsOf(answers(answer), tokens);
    } catch {
      heads = [];
    }
    return answer;
  } finally {
    void deps.record(scope, heads).catch(() => {});
  }
}

/** A room's newest window from its `39006` bounds, or null without one. */
export function roomWindowAnswer(
  roomId: string,
  events: readonly RelayEvent[],
): StoreHeadAnswer | null {
  // The newest window's bounds are keyed `<room>:head`.
  const key = `${roomId.toLowerCase()}:head`;
  const all = events.filter((event) => event.kind === WINDOW_BOUNDS_KIND);
  const bounds = all.length === 1 ? all[0] : undefined;
  if (!bounds?.tags.some((tag) => tag[0] === "d" && tag[1] === key)) {
    return null;
  }
  try {
    const content = JSON.parse(bounds.content) as {
      has_more?: unknown;
      next_cursor?: { created_at?: unknown; id?: unknown } | null;
    };
    const cursor = content.next_cursor;
    const nextCursor =
      typeof cursor?.created_at === "number" && typeof cursor.id === "string"
        ? { createdAt: cursor.created_at, eventId: cursor.id }
        : null;
    if (typeof content.has_more !== "boolean") return null;
    if (content.has_more !== (nextCursor !== null)) return null;
    return {
      roomId,
      rootId: null,
      hasMore: content.has_more,
      nextCursor,
      events: events.filter((event) => event !== bounds),
    };
  } catch {
    return null;
  }
}
