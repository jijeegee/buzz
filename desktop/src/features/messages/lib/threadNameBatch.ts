import type { RelaySubscriptionFilter } from "@/shared/api/relayClientShared";
import type { RelayEvent } from "@/shared/api/types";
import { KIND_THREAD_NAME } from "@/shared/constants/kinds";
import { selectThreadName } from "./threadName";

/** Names asked for within this window share one read. */
export const THREAD_NAME_BATCH_MS = 50;
/** The relay accepts at most 128 `#h` values per REQ. */
export const THREAD_NAME_BATCH_ROOMS = 128;
/** Thread heads per read. */
export const THREAD_NAME_BATCH_IDS = 100;
/** Events per page. */
export const THREAD_NAME_PAGE_LIMIT = 200;
/** Pages per read before the rest is left unresolved. */
export const THREAD_NAME_MAX_PAGES = 10;

export type ThreadNameBatchDeps = {
  fetchEvents: (filter: RelaySubscriptionFilter) => Promise<RelayEvent[]>;
  /** Resolves once the room's live events arrive, so nothing falls between. */
  whenRoomCovered: (roomId: string) => Promise<void>;
  setTimeout?: (fn: () => void, ms: number) => unknown;
};

type Request = {
  channelId: string;
  threadId: string;
  waiters: {
    resolve: (event: RelayEvent | null) => void;
    reject: (error: unknown) => void;
  }[];
};

const keyOf = (channelId: string, threadId: string) =>
  `${channelId}\u0000${threadId}`;

/**
 * Thread names for many rows in a few reads, instead of one read per row.
 *
 * Names asked for within `THREAD_NAME_BATCH_MS` are read together, once
 * their rooms are covered by the live bundle (`whenRoomCovered`), so a rename
 * between the read and the live stream cannot be lost. One read is one filter
 * `{kinds:[40009], #h:[≤128 rooms], #e:[≤100 heads], limit:200}`; the relay
 * returns newest first, so the first valid event per (room, head) is the
 * name (`selectThreadName` decides ties the same way). Pages go back with an
 * inclusive `until` and drop heads already named from `#e`. A short page
 * ends the read (the rest have no name); after `THREAD_NAME_MAX_PAGES` the
 * rest resolve to null and callers keep what they hold. A failed read rejects
 * every caller in it.
 */
export class ThreadNameBatcher {
  private queued = new Map<string, Request>();
  private timer: unknown = null;
  private readonly deps: ThreadNameBatchDeps;

  constructor(deps: ThreadNameBatchDeps) {
    this.deps = deps;
  }

  load(channelId: string, threadId: string): Promise<RelayEvent | null> {
    return new Promise((resolve, reject) => {
      const key = keyOf(channelId, threadId);
      let request = this.queued.get(key);
      if (!request) {
        request = { channelId, threadId, waiters: [] };
        this.queued.set(key, request);
      }
      request.waiters.push({ resolve, reject });
      if (this.timer === null) {
        const schedule = this.deps.setTimeout ?? setTimeout;
        this.timer = schedule(() => this.flush(), THREAD_NAME_BATCH_MS);
      }
    });
  }

  private flush() {
    this.timer = null;
    const requests = [...this.queued.values()];
    this.queued.clear();
    for (const bucket of buckets(requests)) void this.read(bucket);
  }

  private async read(bucket: Request[]) {
    try {
      const rooms = [...new Set(bucket.map((request) => request.channelId))];
      await Promise.all(
        rooms.map((roomId) => this.deps.whenRoomCovered(roomId)),
      );
      const names = await this.readPages(bucket);
      for (const request of bucket) {
        const name =
          names.get(keyOf(request.channelId, request.threadId)) ?? null;
        for (const waiter of request.waiters) waiter.resolve(name);
      }
    } catch (error) {
      for (const request of bucket) {
        for (const waiter of request.waiters) waiter.reject(error);
      }
    }
  }

  private async readPages(bucket: Request[]) {
    const names = new Map<string, RelayEvent>();
    const seen = new Set<string>();
    let open = bucket;
    let until: number | undefined;
    for (let page = 0; page < THREAD_NAME_MAX_PAGES; page += 1) {
      const events = await this.deps.fetchEvents({
        kinds: [KIND_THREAD_NAME],
        "#h": [...new Set(open.map((request) => request.channelId))],
        "#e": open.map((request) => request.threadId),
        limit: THREAD_NAME_PAGE_LIMIT,
        ...(until === undefined ? {} : { until }),
      });
      let oldest: number | undefined;
      let fresh = 0;
      for (const event of events) {
        oldest = Math.min(oldest ?? event.created_at, event.created_at);
        if (seen.has(event.id)) continue;
        seen.add(event.id);
        fresh += 1;
        for (const request of open) {
          const key = keyOf(request.channelId, request.threadId);
          const next = selectThreadName(
            names.get(key) ?? null,
            event,
            request.channelId,
            request.threadId,
          );
          if (next) names.set(key, next);
        }
      }
      open = open.filter(
        (request) => !names.has(keyOf(request.channelId, request.threadId)),
      );
      if (events.length < THREAD_NAME_PAGE_LIMIT || open.length === 0) {
        return names;
      }
      if (oldest === undefined) return names;
      // `until` is inclusive: a page of one second only repeats itself.
      until = fresh === 0 || oldest === until ? oldest - 1 : oldest;
    }
    console.warn(
      "[thread-name] page cap reached; names left unresolved:",
      open.length,
    );
    return names;
  }
}

/** Requests grouped into reads of ≤128 rooms and ≤100 heads. */
function buckets(requests: Request[]): Request[][] {
  const sorted = [...requests].sort((a, b) =>
    a.channelId === b.channelId
      ? a.threadId.localeCompare(b.threadId)
      : a.channelId.localeCompare(b.channelId),
  );
  const result: Request[][] = [];
  let current: Request[] = [];
  let rooms = new Set<string>();
  for (const request of sorted) {
    const addsRoom = !rooms.has(request.channelId);
    if (
      current.length >= THREAD_NAME_BATCH_IDS ||
      (addsRoom && rooms.size >= THREAD_NAME_BATCH_ROOMS)
    ) {
      result.push(current);
      current = [];
      rooms = new Set();
    }
    current.push(request);
    rooms.add(request.channelId);
  }
  if (current.length > 0) result.push(current);
  return result;
}
