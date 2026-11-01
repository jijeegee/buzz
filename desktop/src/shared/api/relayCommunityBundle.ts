import type { RelaySubscriptionFilter } from "@/shared/api/relayClientShared";
import type { LiveFiltersHandle } from "@/shared/api/relayLiveFilters";
import { eventMatchesFilter } from "@/shared/api/relayMeBundle";
import type { RelayEvent } from "@/shared/api/types";

/**
 * The community-wide live filters one bundle REQ carries, in order. Shared
 * with mobile through `test-fixtures/community-live-bundle.json`
 * (docs/message-sync.md#community-bundle).
 */
export type CommunityRow = "emoji" | "status" | "catalogs" | "presence";

export const COMMUNITY_ROW_FILTERS: Record<
  CommunityRow,
  RelaySubscriptionFilter
> = {
  emoji: { kinds: [30030], "#d": ["kuzz:custom-emoji"], limit: 0 },
  status: { kinds: [30315], "#d": ["general"], limit: 0 },
  catalogs: { kinds: [30175, 30178], limit: 0 },
  // `authors` is the bundle's presence author set; omitted while empty.
  presence: { kinds: [20001], limit: 0 },
};

export const DESKTOP_COMMUNITY_ROWS: readonly CommunityRow[] = [
  "emoji",
  "status",
  "catalogs",
  "presence",
];

/** The relay's values-per-list limit. */
export const PRESENCE_AUTHOR_CAP = 1_000;
export const PRESENCE_UPDATE_DEBOUNCE_MS = 250;
/** At most one presence re-REQ per this long. */
export const PRESENCE_UPDATE_INTERVAL_MS = 30_000;
export const COMMUNITY_OPEN_DELAY_MS = 250;
const REOPEN_BASE_MS = 1_000;
const REOPEN_MAX_MS = 60_000;
const LISTENER_SEEN_LIMIT = 2_000;
const LISTENER_KEYS = new Set(["kinds", "#d", "limit"]);

function sameTag(left?: string[], right?: string[]) {
  if (!left || !right) return left === right;
  return JSON.stringify([...left].sort()) === JSON.stringify([...right].sort());
}

/**
 * The row a listener filter rides: live only (`limit: 0`), kinds a subset of
 * one row's and the same `#d`. A filter with `authors` (it would silently get
 * only the tracked set), any other key, or history gets null: its own REQ.
 */
export function communityScopeOf(
  filter: RelaySubscriptionFilter,
  rows: readonly CommunityRow[] = DESKTOP_COMMUNITY_ROWS,
): CommunityRow | null {
  if (filter.limit !== 0) return null;
  if (Object.keys(filter).some((key) => !LISTENER_KEYS.has(key))) return null;
  const kinds = filter.kinds;
  if (!kinds || kinds.length === 0) return null;
  for (const row of rows) {
    const rowFilter = COMMUNITY_ROW_FILTERS[row];
    if (!kinds.every((kind) => rowFilter.kinds.includes(kind))) continue;
    if (!sameTag(filter["#d"], rowFilter["#d"])) continue;
    return row;
  }
  return null;
}

export type CommunityStatus = "ready" | "retrying";

export type CommunityListenerOptions = {
  /** "ready" when the REQ is live (again), "retrying" while it is down. */
  onStatus?: (status: CommunityStatus) => void;
  /** The session reset: the author set is empty; add your keys again. */
  onReset?: () => void;
};

type Listener = CommunityListenerOptions & {
  filter: RelaySubscriptionFilter;
  onEvent: (event: RelayEvent) => void;
  seen: Map<string, true>;
};

export type CommunityBundleDependencies = {
  rows: readonly CommunityRow[];
  open: (
    filters: RelaySubscriptionFilter[],
    onEvent: (event: RelayEvent) => void,
    onStatus: (status: CommunityStatus) => void,
    signal: AbortSignal,
    /** The session dropped the REQ for good (terminal CLOSED, teardown). */
    onGone: () => void,
  ) => Promise<LiveFiltersHandle>;
  setTimeout: (fn: () => void, ms: number) => number;
  clearTimeout: (id: number) => void;
  nowMs: () => number;
  warn?: (message: string) => void;
};

function deliver(listener: Listener, event: RelayEvent) {
  if (listener.seen.has(event.id)) return;
  if (!eventMatchesFilter(event, listener.filter)) return;
  listener.seen.set(event.id, true);
  if (listener.seen.size > LISTENER_SEEN_LIMIT) {
    const oldest = listener.seen.keys().next().value;
    if (oldest !== undefined) listener.seen.delete(oldest);
  }
  listener.onEvent(event);
}

/**
 * Every community-wide live subscription on one REQ: the fixed rows above
 * plus presence for a growing author set. Listeners keep their filters and
 * get the events they match, once.
 *
 * - Opens 250 ms after its first listener (launch demand rides the opening
 *   REQ) and closes one task after its last.
 * - The author set lives for the session: lowercased, sorted, grow-only,
 *   capped at 1000 (past the cap, keys are refused once with a warning and
 *   the caller keeps polling them). New keys go out 250 ms after the first
 *   one, but at most one re-REQ per 30 s; the session sends it only after
 *   the previous REQ's EOSE (`subscribeLiveFilters`).
 * - When the session drops the REQ (terminal CLOSED, teardown) or opening
 *   fails, the bundle reopens with backoff (1 s up to 60 s); listeners stay
 *   and hear "retrying", then "ready".
 * - `reset` (community switch, sign-out) clears the set and tells listeners
 *   to add their keys again.
 */
export class CommunityLiveBundle {
  private readonly deps: CommunityBundleDependencies;
  private readonly listeners = new Set<Listener>();
  private readonly authors = new Set<string>();
  private capWarned = false;
  private handle: LiveFiltersHandle | null = null;
  private controller: AbortController | null = null;
  private state: "closed" | "scheduled" | "opening" | "open" | "backoff" =
    "closed";
  private generation = 0;
  private openTimer: number | null = null;
  private reopenTimer: number | null = null;
  private updateTimer: number | null = null;
  private reopenAttempt = 0;
  private lastUpdateAt = Number.NEGATIVE_INFINITY;
  /** The author set the last REQ carried. */
  private sentAuthors = "";
  private openWaiters: (() => void)[] = [];

  constructor(deps: CommunityBundleDependencies) {
    this.deps = deps;
  }

  /**
   * Listen with `filter` (see `communityScopeOf`). Resolves with the stop
   * function once the first open attempt settled; a failed one keeps the
   * listener and retries.
   */
  async subscribe(
    filter: RelaySubscriptionFilter,
    onEvent: (event: RelayEvent) => void,
    options: CommunityListenerOptions = {},
  ): Promise<() => Promise<void>> {
    if (!communityScopeOf(filter, this.deps.rows)) {
      throw new Error("Filter is not a community bundle filter.");
    }
    const listener: Listener = { ...options, filter, onEvent, seen: new Map() };
    this.listeners.add(listener);
    let stopped = false;
    const stop = async () => {
      if (stopped) return;
      stopped = true;
      this.listeners.delete(listener);
      if (this.listeners.size > 0) return;
      queueMicrotask(() => {
        if (this.listeners.size === 0) this.close();
      });
    };
    if (this.state === "closed") this.scheduleOpen(COMMUNITY_OPEN_DELAY_MS);
    if (this.state === "scheduled" || this.state === "opening") {
      await new Promise<void>((resolve) => this.openWaiters.push(resolve));
    }
    return stop;
  }

  /**
   * Add people to the presence author set. Returns the keys past the cap,
   * which the caller keeps polling.
   */
  addPresenceAuthors(pubkeys: Iterable<string>): string[] {
    const refused: string[] = [];
    let added = false;
    for (const raw of pubkeys) {
      const pubkey = raw.toLowerCase();
      if (this.authors.has(pubkey)) continue;
      if (this.authors.size >= PRESENCE_AUTHOR_CAP) {
        refused.push(pubkey);
        continue;
      }
      this.authors.add(pubkey);
      added = true;
    }
    if (refused.length > 0 && !this.capWarned) {
      this.capWarned = true;
      this.deps.warn?.(
        `Presence author set is full (${PRESENCE_AUTHOR_CAP}); polling the rest.`,
      );
    }
    if (added) this.scheduleUpdate();
    return refused;
  }

  /** Whether `pubkey` is in the presence author set. */
  tracksPresenceOf(pubkey: string) {
    return this.authors.has(pubkey.toLowerCase());
  }

  /** The REQ's filters right now (the presence row only with authors). */
  currentFilters(): RelaySubscriptionFilter[] {
    const filters: RelaySubscriptionFilter[] = [];
    for (const row of this.deps.rows) {
      const filter = COMMUNITY_ROW_FILTERS[row];
      if (row !== "presence") {
        filters.push({ ...filter });
      } else if (this.authors.size > 0) {
        filters.push({ ...filter, authors: [...this.authors].sort() });
      }
    }
    return filters;
  }

  /** Session reset: clear the author set and reopen for current listeners. */
  reset() {
    this.close();
    this.authors.clear();
    this.capWarned = false;
    this.lastUpdateAt = Number.NEGATIVE_INFINITY;
    for (const listener of [...this.listeners]) listener.onReset?.();
    if (this.listeners.size > 0 && this.state === "closed") {
      this.scheduleOpen(COMMUNITY_OPEN_DELAY_MS);
    }
  }

  private authorsKey() {
    return [...this.authors].sort().join(",");
  }

  private notify(status: CommunityStatus) {
    for (const listener of [...this.listeners]) listener.onStatus?.(status);
  }

  private releaseOpenWaiters() {
    const waiters = this.openWaiters;
    this.openWaiters = [];
    for (const resolve of waiters) resolve();
  }

  private scheduleOpen(delayMs: number) {
    this.state = "scheduled";
    const generation = this.generation;
    this.openTimer = this.deps.setTimeout(() => {
      this.openTimer = null;
      if (generation !== this.generation) return;
      if (this.listeners.size === 0) {
        this.close();
        return;
      }
      this.open();
    }, delayMs);
  }

  private open() {
    const generation = ++this.generation;
    this.state = "opening";
    const controller = new AbortController();
    this.controller = controller;
    this.sentAuthors = this.authorsKey();
    const isCurrent = () => generation === this.generation;
    this.deps
      .open(
        this.currentFilters(),
        (event) => {
          if (!isCurrent()) return;
          for (const listener of [...this.listeners]) deliver(listener, event);
        },
        (status) => {
          if (isCurrent()) this.notify(status);
        },
        controller.signal,
        () => {
          if (isCurrent()) this.backoff();
        },
      )
      .then(
        (handle) => {
          if (!isCurrent()) {
            void handle.close().catch(() => {});
            return;
          }
          this.handle = handle;
          this.state = "open";
          this.reopenAttempt = 0;
          this.releaseOpenWaiters();
          if (this.authorsKey() !== this.sentAuthors) this.scheduleUpdate();
        },
        (error: unknown) => {
          if (!isCurrent()) return;
          console.error("Failed to open the community bundle", error);
          this.backoff();
          this.releaseOpenWaiters();
        },
      );
  }

  private backoff() {
    this.dropRequest();
    this.releaseOpenWaiters();
    if (this.listeners.size === 0) {
      this.state = "closed";
      return;
    }
    this.state = "backoff";
    this.notify("retrying");
    const delay = Math.min(
      REOPEN_BASE_MS * 2 ** this.reopenAttempt,
      REOPEN_MAX_MS,
    );
    this.reopenAttempt += 1;
    const generation = this.generation;
    this.reopenTimer = this.deps.setTimeout(() => {
      this.reopenTimer = null;
      if (generation !== this.generation) return;
      if (this.listeners.size === 0) {
        this.state = "closed";
        return;
      }
      this.open();
    }, delay);
  }

  private scheduleUpdate() {
    if (this.state !== "open" || this.updateTimer !== null) return;
    const delay = Math.max(
      PRESENCE_UPDATE_DEBOUNCE_MS,
      this.lastUpdateAt + PRESENCE_UPDATE_INTERVAL_MS - this.deps.nowMs(),
    );
    this.updateTimer = this.deps.setTimeout(() => {
      this.updateTimer = null;
      if (this.state !== "open" || !this.handle) return;
      const key = this.authorsKey();
      if (key === this.sentAuthors) return;
      this.sentAuthors = key;
      this.lastUpdateAt = this.deps.nowMs();
      void this.handle.setFilters(this.currentFilters()).catch((error) => {
        console.error("Failed to update the community bundle", error);
      });
    }, delay);
  }

  /** Stop timers and the REQ; listeners and the author set stay. */
  private dropRequest() {
    this.generation += 1;
    for (const timer of [this.openTimer, this.reopenTimer, this.updateTimer]) {
      if (timer !== null) this.deps.clearTimeout(timer);
    }
    this.openTimer = null;
    this.reopenTimer = null;
    this.updateTimer = null;
    this.controller?.abort();
    this.controller = null;
    const handle = this.handle;
    this.handle = null;
    void handle?.close().catch(() => {});
  }

  private close() {
    this.dropRequest();
    this.state = "closed";
    this.reopenAttempt = 0;
    this.releaseOpenWaiters();
  }
}
