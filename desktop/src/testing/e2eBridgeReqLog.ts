/**
 * Wire log for the mock relay sockets: every client REQ/CLOSE, every
 * relay-sent CLOSED, and socket open/close, in order. The live REQ budget
 * spec derives the open subscription set from this log rather than from the
 * mock's own `socket.subscriptions`, which only ever holds `live-` ids.
 *
 * The log is cumulative for a page load. Callers slice it by `seq` instead of
 * resetting it, because resetting would also forget which subscriptions are
 * still open.
 */

export type ReqLogEntry = {
  seq: number;
  at: number;
  socketId: number;
} & (
  | { type: "REQ"; subId: string; filters: unknown[] }
  | { type: "CLOSE"; subId: string }
  | { type: "CLOSED"; subId: string; reason: string }
  | { type: "CONNECT" }
  | { type: "DISCONNECT" }
);

/** `live` stays open by design; `oneShot` closes after EOSE or first event. */
export type SubIdClass = "live" | "stream" | "oneShot" | "unclassified";

export type OpenSubscription = {
  socketId: number;
  subId: string;
  subIdClass: SubIdClass;
  filters: unknown[];
  /** `subscriptionSignature(filters)`, for budget fixtures and diffs. */
  signature: string;
};

// Longest prefix first: `observer-history-` must not be read as unknown.
const ONE_SHOT_PREFIXES = ["observer-history-", "history-", "first-"];

export function classifySubId(subId: string): SubIdClass {
  if (subId.startsWith("live-")) return "live";
  // The user stream is its own live REQ (one per connection).
  if (subId.startsWith("stream-")) return "stream";
  if (ONE_SHOT_PREFIXES.some((prefix) => subId.startsWith(prefix))) {
    return "oneShot";
  }
  return "unclassified";
}

/** Open = REQ'd and not yet closed by CLOSE, CLOSED, or socket teardown. */
export function openSubscriptionsFromLog(
  log: readonly ReqLogEntry[],
): OpenSubscription[] {
  const open = new Map<string, OpenSubscription>();
  for (const entry of log) {
    if (entry.type === "REQ") {
      const key = `${entry.socketId}:${entry.subId}`;
      // A re-REQ on the same id replaces the earlier filters.
      open.delete(key);
      open.set(key, {
        socketId: entry.socketId,
        subId: entry.subId,
        subIdClass: classifySubId(entry.subId),
        filters: entry.filters,
        signature: subscriptionSignature(entry.filters),
      });
    } else if (entry.type === "CLOSE" || entry.type === "CLOSED") {
      open.delete(`${entry.socketId}:${entry.subId}`);
    } else if (entry.type === "DISCONNECT") {
      for (const [key, sub] of open) {
        if (sub.socketId === entry.socketId) open.delete(key);
      }
    }
  }
  return [...open.values()];
}

const ID_LIKE =
  /^([0-9a-f]{64}|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$/i;

function isIdLike(value: unknown) {
  return typeof value === "string" && ID_LIKE.test(value);
}

/** Ids, pubkeys and room ids count; other values (`general`) stay. */
function describeList(value: unknown): string {
  if (!Array.isArray(value)) return "?";
  if (value.every(isIdLike)) return String(value.length);
  return `[${value.map((item) => (isIdLike(item) ? "*" : String(item))).join(",")}]`;
}

function isPresenceFilter(record: Record<string, unknown>) {
  const kinds = record.kinds;
  return Array.isArray(kinds) && kinds.length === 1 && kinds[0] === 20001;
}

/**
 * Normalised filter shape with ids, pubkeys and timestamps stripped: kinds
 * (sorted) and limit keep their values, id lists keep only their length,
 * other tag values stay. The presence author set grows with what the screen
 * shows, so its size is `authors:+`. Same rules as mobile's
 * `test/helpers/scripted_relay.dart`.
 */
export function filterSignature(filter: unknown): string {
  if (!filter || typeof filter !== "object") return String(filter);
  const record = filter as Record<string, unknown>;
  return Object.keys(record)
    .sort()
    .map((key) => {
      const value = record[key];
      if (key === "kinds" && Array.isArray(value)) {
        return `kinds:[${[...value].sort((a, b) => a - b).join(",")}]`;
      }
      if (key === "limit") return `limit:${String(value)}`;
      if (key === "since" || key === "until" || key === "search") return key;
      if (key === "authors" && isPresenceFilter(record)) return "authors:+";
      return `${key}:${describeList(value)}`;
    })
    .join("|");
}

/** One REQ may carry several filters; they are joined in sorted order. */
export function subscriptionSignature(filters: readonly unknown[]): string {
  return filters.map(filterSignature).sort().join(" + ");
}

let entries: ReqLogEntry[] = [];
let nextSeq = 1;

type NewEntry = ReqLogEntry extends infer E
  ? E extends ReqLogEntry
    ? Omit<E, "seq" | "at">
    : never
  : never;

function push(entry: NewEntry) {
  entries.push({ ...entry, seq: nextSeq++, at: Date.now() } as ReqLogEntry);
}

export function recordReq(socketId: number, subId: string, filters: unknown[]) {
  push({ type: "REQ", socketId, subId, filters });
}

export function recordClose(socketId: number, subId: string) {
  push({ type: "CLOSE", socketId, subId });
}

export function recordRelayClosed(
  socketId: number,
  subId: string,
  reason: string,
) {
  push({ type: "CLOSED", socketId, subId, reason });
}

export function recordSocketOpened(socketId: number) {
  push({ type: "CONNECT", socketId });
}

export function recordSocketClosed(socketId: number) {
  push({ type: "DISCONNECT", socketId });
}

export function getReqLog(): ReqLogEntry[] {
  return entries.slice();
}

export function getOpenSubscriptions(): OpenSubscription[] {
  return openSubscriptionsFromLog(entries);
}

export function resetReqLog() {
  entries = [];
  nextSeq = 1;
}
