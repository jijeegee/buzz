import {
  isChatStateUnavailable,
  isPermanentChatStateError,
  refusedMarkReadItem,
  type ChatFollowState,
  type ChatMarkReadItem,
} from "@/shared/api/chatState";

/** Reads are collected this long before one `mark_read` request. */
export const MARK_READ_BATCH_MS = 1_000;
/** Most items per `mark_read` request (relay `MAX_MARK_READ_ITEMS`). */
export const MARK_READ_MAX_ITEMS = 100;
/** Most contexts kept waiting; the oldest positions are dropped beyond it. */
export const MARK_READ_MAX_PENDING = 500;
/** Consecutive transient failures before retries wait for the next read. */
const MAX_RETRIES = 6;
const RETRY_MAX_MS = 60_000;
const MAX_SENT_CONTEXTS = 2_000;

const UUID_RE =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const EVENT_ID_RE = /^[0-9a-f]{64}$/i;

export type ChatStateWriteCapability = "unknown" | "on" | "off";

type TimerHost = {
  setTimeout: (handler: () => void, ms: number) => number;
  clearTimeout: (id: number) => void;
};

export type ChatStateWriterDeps = {
  markRead: (items: readonly ChatMarkReadItem[]) => Promise<unknown>;
  follow: (root: string, state: ChatFollowState) => Promise<unknown>;
  /** Whether the relay keeps chat state; "unknown" buffers writes. */
  capability: () => ChatStateWriteCapability;
  /** The relay answered "unavailable": turn the capability off. */
  onUnavailable: () => void;
  timers?: TimerHost;
};

/**
 * A ReadStateManager context as a `mark_read` item. `msg:` and other schemes
 * have no server position; malformed ids would fail the whole request.
 */
export function markReadItemForContext(
  contextId: string,
  unixTimestamp: number,
): ChatMarkReadItem | null {
  const at = Math.floor(unixTimestamp);
  if (!Number.isFinite(at) || at <= 0) return null;
  if (contextId.startsWith("thread:")) {
    const root = contextId.slice("thread:".length);
    return EVENT_ID_RE.test(root)
      ? { kind: "thread", id: root.toLowerCase(), at }
      : null;
  }
  return UUID_RE.test(contextId)
    ? { kind: "channel", id: contextId.toLowerCase(), at }
    : null;
}

function itemKey(item: ChatMarkReadItem): string {
  return `${item.kind}:${item.id}`;
}

/**
 * Mirrors this device's reads and follow choices to the relay's chat state.
 *
 * Reads are merged per context (newest position wins), sent in batches after
 * MARK_READ_BATCH_MS, and flushed at once when the app hides. A refused item
 * (not a member) is dropped and the rest resent; malformed requests are
 * dropped; network failures are retried with backoff. "Unavailable" turns the
 * capability off and drops everything queued. Local read state stays the
 * source of truth for this release, so nothing here is persisted. Positions
 * and follows are never logged.
 */
export class ChatStateWriter {
  private readonly deps: ChatStateWriterDeps;
  private readonly timers: TimerHost;
  private pending = new Map<string, ChatMarkReadItem>();
  /** Newest position sent (or in flight) per context. */
  private sent = new Map<string, number>();
  private pendingFollows = new Map<string, ChatFollowState>();
  private timer: number | null = null;
  private followTimer: number | null = null;
  private inFlight: Promise<void> | null = null;
  private followInFlight: Promise<void> | null = null;
  private failures = 0;
  private followFailures = 0;
  private disposed = false;

  constructor(deps: ChatStateWriterDeps) {
    this.deps = deps;
    this.timers = deps.timers ?? window;
  }

  /** Queue a read position from ReadStateManager.markContextRead. */
  queueRead(contextId: string, unixTimestamp: number): void {
    if (this.disposed) return;
    const capability = this.deps.capability();
    if (capability === "off") return;
    const item = markReadItemForContext(contextId, unixTimestamp);
    if (!item) return;
    const key = itemKey(item);
    if (item.at <= (this.sent.get(key) ?? 0)) return;
    const queued = this.pending.get(key);
    if (queued && queued.at >= item.at) return;
    this.pending.delete(key);
    this.pending.set(key, item);
    this.trimPending();
    if (capability === "on") this.schedule(MARK_READ_BATCH_MS);
  }

  /** Mirror an explicit Follow / Unfollow. The newest choice per thread wins. */
  setFollow(rootId: string, state: ChatFollowState): void {
    if (this.disposed || this.deps.capability() === "off") return;
    if (!EVENT_ID_RE.test(rootId)) return;
    const root = rootId.toLowerCase();
    this.pendingFollows.delete(root);
    this.pendingFollows.set(root, state);
    this.followFailures = 0;
    if (this.deps.capability() === "on") this.scheduleFollows(0);
  }

  /** The capability changed: send what was buffered, or drop it. */
  capabilityChanged(): void {
    const capability = this.deps.capability();
    if (capability === "off") {
      this.clear();
    } else if (capability === "on") {
      this.failures = 0;
      this.followFailures = 0;
      if (this.pending.size > 0) this.schedule(MARK_READ_BATCH_MS);
      if (this.pendingFollows.size > 0) this.scheduleFollows(0);
    }
  }

  /** Send everything queued now (app hidden or closing). */
  flush(): Promise<void> {
    this.cancelTimer();
    if (this.disposed || this.deps.capability() !== "on") {
      return Promise.resolve();
    }
    void this.flushFollows();
    if (this.inFlight) return this.inFlight;
    if (this.pending.size === 0) return Promise.resolve();
    const run = this.sendBatch().finally(() => {
      if (this.inFlight === run) this.inFlight = null;
    });
    this.inFlight = run;
    return run;
  }

  dispose(): void {
    this.disposed = true;
    this.clear();
  }

  private clear() {
    this.cancelTimer();
    if (this.followTimer !== null) {
      this.timers.clearTimeout(this.followTimer);
      this.followTimer = null;
    }
    this.pending.clear();
    this.pendingFollows.clear();
  }

  private trimPending() {
    while (this.pending.size > MARK_READ_MAX_PENDING) {
      let oldest: string | null = null;
      let oldestAt = Number.POSITIVE_INFINITY;
      for (const [key, item] of this.pending) {
        if (item.at < oldestAt) {
          oldest = key;
          oldestAt = item.at;
        }
      }
      if (oldest === null) return;
      this.pending.delete(oldest);
    }
  }

  private cancelTimer() {
    if (this.timer !== null) {
      this.timers.clearTimeout(this.timer);
      this.timer = null;
    }
  }

  private schedule(delayMs: number) {
    if (this.timer !== null || this.disposed) return;
    this.timer = this.timers.setTimeout(() => {
      this.timer = null;
      void this.flush();
    }, delayMs);
  }

  private requeue(items: readonly ChatMarkReadItem[]) {
    for (const item of items) {
      const key = itemKey(item);
      const queued = this.pending.get(key);
      if (!queued || queued.at < item.at) this.pending.set(key, item);
    }
    this.trimPending();
  }

  private async sendBatch(): Promise<void> {
    let items = [...this.pending.values()].slice(0, MARK_READ_MAX_ITEMS);
    for (const item of items) this.pending.delete(itemKey(item));
    const previous = new Map(
      items.map((item) => [itemKey(item), this.sent.get(itemKey(item))]),
    );
    for (const item of items) {
      this.sent.delete(itemKey(item));
      this.sent.set(itemKey(item), item.at);
    }
    // Keep the dedupe memory bounded; the relay ignores older positions anyway.
    for (const key of this.sent.keys()) {
      if (this.sent.size <= MAX_SENT_CONTEXTS) break;
      this.sent.delete(key);
    }
    const forget = (dropped: readonly ChatMarkReadItem[]) => {
      for (const item of dropped) {
        const before = previous.get(itemKey(item));
        if (before === undefined) this.sent.delete(itemKey(item));
        else this.sent.set(itemKey(item), before);
      }
    };

    // Each refusal drops one item, so this ends within MARK_READ_MAX_ITEMS tries.
    while (items.length > 0) {
      try {
        await this.deps.markRead(items);
        this.failures = 0;
        break;
      } catch (error) {
        if (this.disposed) return;
        if (isChatStateUnavailable(error)) {
          forget(items);
          this.deps.onUnavailable();
          this.clear();
          return;
        }
        const refused = refusedMarkReadItem(error);
        if (refused !== null && refused < items.length) {
          forget([items[refused]]);
          items = items.filter((_, index) => index !== refused);
          continue;
        }
        forget(items);
        if (isPermanentChatStateError(error)) {
          console.warn("chat-state mark_read rejected; dropping the batch");
          break;
        }
        this.requeue(items);
        this.failures += 1;
        if (this.failures <= MAX_RETRIES) {
          this.schedule(
            Math.min(RETRY_MAX_MS, MARK_READ_BATCH_MS * 2 ** this.failures),
          );
        }
        // Past MAX_RETRIES the next read (or reconnect) schedules the retry.
        return;
      }
    }
    if (this.pending.size > 0) this.schedule(MARK_READ_BATCH_MS);
  }

  private scheduleFollows(delayMs: number) {
    if (this.followTimer !== null || this.disposed) return;
    this.followTimer = this.timers.setTimeout(() => {
      this.followTimer = null;
      void this.flushFollows();
    }, delayMs);
  }

  private flushFollows(): Promise<void> {
    if (this.followInFlight) return this.followInFlight;
    if (this.pendingFollows.size === 0 || this.deps.capability() !== "on") {
      return Promise.resolve();
    }
    const run = this.sendFollows().finally(() => {
      if (this.followInFlight === run) this.followInFlight = null;
      if (this.pendingFollows.size > 0 && this.followFailures === 0) {
        this.scheduleFollows(0);
      }
    });
    this.followInFlight = run;
    return run;
  }

  private async sendFollows(): Promise<void> {
    for (const [root, state] of [...this.pendingFollows]) {
      try {
        await this.deps.follow(root, state);
        if (this.pendingFollows.get(root) === state) {
          this.pendingFollows.delete(root);
        }
        this.followFailures = 0;
      } catch (error) {
        if (this.disposed) return;
        if (isChatStateUnavailable(error)) {
          this.deps.onUnavailable();
          this.clear();
          return;
        }
        if (isPermanentChatStateError(error)) {
          // Not a member, or a reply rather than a root: nothing to store.
          if (this.pendingFollows.get(root) === state) {
            this.pendingFollows.delete(root);
          }
          continue;
        }
        this.followFailures += 1;
        if (this.followFailures <= MAX_RETRIES) {
          this.scheduleFollows(
            Math.min(
              RETRY_MAX_MS,
              MARK_READ_BATCH_MS * 2 ** this.followFailures,
            ),
          );
        }
        return;
      }
    }
  }
}
