import {
  isChatStateAccountChanged,
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
/**
 * A chat-state ping this soon after one of this device's own writes is that
 * write's echo: the write's response already patched the list.
 */
export const OWN_WRITE_PING_WINDOW_MS = 3_000;
/** Consecutive transient failures before retries wait for the next read. */
const MAX_RETRIES = 6;
const RETRY_MAX_MS = 60_000;
const MAX_SENT_CONTEXTS = 2_000;
const MAX_REFUSED_CONTEXTS = 2_000;

const UUID_RE =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const EVENT_ID_RE = /^[0-9a-f]{64}$/i;

export type ChatStateWriteCapability = "unknown" | "on" | "off";

type TimerHost = {
  setTimeout: (handler: () => void, ms: number) => number;
  clearTimeout: (id: number) => void;
};

/** A write the relay accepted, with its response, for patching the list. */
export type ChatStateWritten =
  | { type: "mark_read"; response: unknown }
  | { type: "thread_follow"; response: unknown };

export type ChatStateWriterDeps = {
  /**
   * Send one `mark_read` request. The caller binds it to the account the
   * writer was made for; a request for another account fails with
   * "chat_state_account_changed".
   */
  markRead: (
    items: readonly ChatMarkReadItem[],
    unreadMark: boolean,
  ) => Promise<unknown>;
  follow: (root: string, state: ChatFollowState) => Promise<unknown>;
  /** Whether the relay keeps chat state; "unknown" buffers writes. */
  capability: () => ChatStateWriteCapability;
  /** The relay answered "unavailable": turn the capability off. */
  onUnavailable: () => void;
  /** The relay stored a write; its response can patch the loaded list. */
  onWritten?: (written: ChatStateWritten) => void;
  /** True for a room (or thread) the user is known not to be a member of. */
  isKnownNonMember?: (item: ChatMarkReadItem) => boolean;
  now?: () => number;
  timers?: TimerHost;
};

type MarkTarget = Pick<ChatMarkReadItem, "kind" | "id">;

function markTargetForContext(contextId: string): MarkTarget | null {
  if (contextId.startsWith("thread:")) {
    const root = contextId.slice("thread:".length);
    return EVENT_ID_RE.test(root)
      ? { kind: "thread", id: root.toLowerCase() }
      : null;
  }
  return UUID_RE.test(contextId)
    ? { kind: "channel", id: contextId.toLowerCase() }
    : null;
}

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
  const target = markTargetForContext(contextId);
  return target ? { ...target, at } : null;
}

function itemKey(item: MarkTarget): string {
  return `${item.kind}:${item.id}`;
}

type BatchOutcome = "done" | "later" | "stop";

/**
 * Mirrors this device's reads, unread marks and follow choices to the
 * relay's chat state, for one account (relay + pubkey).
 *
 * Reads are merged per context (newest position wins), sent in batches after
 * MARK_READ_BATCH_MS, and flushed at once when the app hides; a flush sends
 * everything queued, in chunks of MARK_READ_MAX_ITEMS. A context the relay
 * refuses (not a member) is remembered for the session and never resent; a
 * refused request is retried once without the refused item, and anything
 * left waits for the next batch. Malformed requests are dropped; network
 * failures are retried with backoff. "Unavailable" turns the capability off
 * and drops everything queued. "Account changed" (the active account or
 * relay is no longer this writer's) drops everything and stops the writer:
 * nothing queued for one account is ever sent as another. Local read state
 * stays the source of truth for this release, so nothing here is persisted.
 * Positions and follows are never logged.
 */
export class ChatStateWriter {
  private readonly deps: ChatStateWriterDeps;
  private readonly timers: TimerHost;
  private readonly now: () => number;
  private pending = new Map<string, ChatMarkReadItem>();
  /** Unread marks waiting, sent after the reads with `unread_mark: true`. */
  private pendingUnread = new Map<string, ChatMarkReadItem>();
  /** Newest position sent (or in flight) per context. */
  private sent = new Map<string, number>();
  /** Contexts the relay refused this session (not a member). */
  private refused = new Set<string>();
  private pendingFollows = new Map<string, ChatFollowState>();
  private timer: number | null = null;
  private followTimer: number | null = null;
  private inFlight: Promise<void> | null = null;
  private followInFlight: Promise<void> | null = null;
  private failures = 0;
  private followFailures = 0;
  private writesInFlight = 0;
  private lastWriteAt = Number.NEGATIVE_INFINITY;
  private disposed = false;

  constructor(deps: ChatStateWriterDeps) {
    this.deps = deps;
    this.timers = deps.timers ?? window;
    this.now = deps.now ?? Date.now;
  }

  /** Queue a read position from ReadStateManager.markContextRead. */
  queueRead(contextId: string, unixTimestamp: number): void {
    if (this.disposed) return;
    const capability = this.deps.capability();
    if (capability === "off") return;
    const item = markReadItemForContext(contextId, unixTimestamp);
    if (!item || !this.mayWrite(item)) return;
    const key = itemKey(item);
    // A read clears an unread mark still waiting for the same context.
    this.pendingUnread.delete(key);
    const at = item.at ?? 0;
    if (at <= (this.sent.get(key) ?? 0)) return;
    const queued = this.pending.get(key);
    if (queued && (queued.at ?? 0) >= at) return;
    this.pending.delete(key);
    this.pending.set(key, item);
    this.trimPending();
    if (capability === "on") this.schedule(MARK_READ_BATCH_MS);
  }

  /**
   * An explicit Mark as read follows: send it even at a position already
   * sent, since another device may have marked the context unread since.
   */
  forgetSent(contextId: string): void {
    const target = markTargetForContext(contextId);
    if (target) this.sent.delete(itemKey(target));
  }

  /** Mirror "Mark as unread" for a room or thread context. */
  queueUnread(contextId: string): void {
    if (this.disposed || this.deps.capability() === "off") return;
    const target = markTargetForContext(contextId);
    if (!target || !this.mayWrite(target)) return;
    const key = itemKey(target);
    this.pending.delete(key);
    // The next read, even at the same position, must clear the mark again.
    this.sent.delete(key);
    this.pendingUnread.delete(key);
    this.pendingUnread.set(key, { ...target });
    if (this.deps.capability() === "on") this.schedule(MARK_READ_BATCH_MS);
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
    if (this.disposed) return;
    const capability = this.deps.capability();
    if (capability === "off") {
      this.clear();
    } else if (capability === "on") {
      this.failures = 0;
      this.followFailures = 0;
      if (this.hasPendingMarks()) this.schedule(MARK_READ_BATCH_MS);
      if (this.pendingFollows.size > 0) this.scheduleFollows(0);
    }
  }

  /**
   * Whether a chat-state ping now is explained by this device's own write
   * (one in flight, or one stored within OWN_WRITE_PING_WINDOW_MS).
   */
  explainsPing(): boolean {
    return (
      this.writesInFlight > 0 ||
      this.now() - this.lastWriteAt < OWN_WRITE_PING_WINDOW_MS
    );
  }

  /** Send everything queued now (app hidden or closing). */
  flush(): Promise<void> {
    this.cancelTimer();
    if (this.disposed || this.deps.capability() !== "on") {
      return Promise.resolve();
    }
    void this.flushFollows();
    // A running drain also sends what was queued while it ran.
    if (this.inFlight) return this.inFlight;
    if (!this.hasPendingMarks()) return Promise.resolve();
    const run = this.drain().finally(() => {
      if (this.inFlight === run) this.inFlight = null;
    });
    this.inFlight = run;
    return run;
  }

  /** Drop everything queued and send nothing more (account switch, teardown). */
  dispose(): void {
    this.disposed = true;
    this.clear();
  }

  get isDisposed(): boolean {
    return this.disposed;
  }

  private mayWrite(target: MarkTarget): boolean {
    if (this.refused.has(itemKey(target))) return false;
    return !(this.deps.isKnownNonMember?.({ ...target }) ?? false);
  }

  private hasPendingMarks() {
    return this.pending.size > 0 || this.pendingUnread.size > 0;
  }

  private clear() {
    this.cancelTimer();
    if (this.followTimer !== null) {
      this.timers.clearTimeout(this.followTimer);
      this.followTimer = null;
    }
    this.pending.clear();
    this.pendingUnread.clear();
    this.pendingFollows.clear();
  }

  private refuse(item: MarkTarget) {
    const key = itemKey(item);
    this.refused.delete(key);
    this.refused.add(key);
    for (const oldest of this.refused) {
      if (this.refused.size <= MAX_REFUSED_CONTEXTS) break;
      this.refused.delete(oldest);
    }
    this.pending.delete(key);
    this.pendingUnread.delete(key);
  }

  private trimPending() {
    while (this.pending.size > MARK_READ_MAX_PENDING) {
      let oldest: string | null = null;
      let oldestAt = Number.POSITIVE_INFINITY;
      for (const [key, item] of this.pending) {
        if ((item.at ?? 0) < oldestAt) {
          oldest = key;
          oldestAt = item.at ?? 0;
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

  private requeue(items: readonly ChatMarkReadItem[], unreadMark: boolean) {
    for (const item of items) {
      const key = itemKey(item);
      if (unreadMark) {
        if (!this.pending.has(key) && !this.pendingUnread.has(key)) {
          this.pendingUnread.set(key, item);
        }
        continue;
      }
      if (this.pendingUnread.has(key)) continue;
      const queued = this.pending.get(key);
      if (!queued || (queued.at ?? 0) < (item.at ?? 0)) {
        this.pending.set(key, item);
      }
    }
    this.trimPending();
  }

  private async write<T>(send: () => Promise<T>): Promise<T> {
    this.writesInFlight += 1;
    try {
      const response = await send();
      this.lastWriteAt = this.now();
      return response;
    } finally {
      this.writesInFlight -= 1;
    }
  }

  /** Account changed under this writer: nothing more may be sent. */
  private stopForAccountChange() {
    this.dispose();
  }

  private async drain(): Promise<void> {
    while (
      !this.disposed &&
      this.deps.capability() === "on" &&
      this.hasPendingMarks()
    ) {
      const outcome = await this.sendBatch();
      if (outcome === "later") {
        this.schedule(MARK_READ_BATCH_MS);
        return;
      }
      if (outcome === "stop") return;
    }
  }

  private async sendBatch(): Promise<BatchOutcome> {
    // Reads first, then unread marks: a mark set after a read must win.
    const unreadMark = this.pending.size === 0;
    const source = unreadMark ? this.pendingUnread : this.pending;
    let items = [...source.values()].slice(0, MARK_READ_MAX_ITEMS);
    for (const item of items) source.delete(itemKey(item));
    const previous = new Map(
      items.map((item) => [itemKey(item), this.sent.get(itemKey(item))]),
    );
    if (!unreadMark) {
      for (const item of items) {
        this.sent.delete(itemKey(item));
        this.sent.set(itemKey(item), item.at ?? 0);
      }
      // Keep the dedupe memory bounded; the relay ignores older positions anyway.
      for (const key of this.sent.keys()) {
        if (this.sent.size <= MAX_SENT_CONTEXTS) break;
        this.sent.delete(key);
      }
    }
    const forget = (dropped: readonly ChatMarkReadItem[]) => {
      if (unreadMark) return;
      for (const item of dropped) {
        const before = previous.get(itemKey(item));
        if (before === undefined) this.sent.delete(itemKey(item));
        else this.sent.set(itemKey(item), before);
      }
    };

    let retried = false;
    while (items.length > 0) {
      try {
        const response = await this.write(() =>
          this.deps.markRead(items, unreadMark),
        );
        this.failures = 0;
        if (!this.disposed) {
          this.deps.onWritten?.({ type: "mark_read", response });
        }
        return "done";
      } catch (error) {
        if (this.disposed) return "stop";
        if (isChatStateAccountChanged(error)) {
          this.stopForAccountChange();
          return "stop";
        }
        if (isChatStateUnavailable(error)) {
          forget(items);
          this.deps.onUnavailable();
          this.clear();
          return "stop";
        }
        const refused = refusedMarkReadItem(error);
        if (refused !== null && refused < items.length) {
          const item = items[refused];
          forget([item]);
          this.refuse(item);
          items = items.filter((_, index) => index !== refused);
          if (items.length === 0) return "done";
          if (!retried) {
            retried = true;
            continue;
          }
          // Refused twice: the rest waits for the next batch rather than
          // one request per refused room.
          forget(items);
          this.requeue(items, unreadMark);
          return "later";
        }
        forget(items);
        if (isPermanentChatStateError(error)) {
          console.warn("chat-state mark_read rejected; dropping the batch");
          return "done";
        }
        this.requeue(items, unreadMark);
        this.failures += 1;
        if (this.failures <= MAX_RETRIES) {
          this.schedule(
            Math.min(RETRY_MAX_MS, MARK_READ_BATCH_MS * 2 ** this.failures),
          );
        }
        // Past MAX_RETRIES the next read (or reconnect) schedules the retry.
        return "stop";
      }
    }
    return "done";
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
    if (
      this.disposed ||
      this.pendingFollows.size === 0 ||
      this.deps.capability() !== "on"
    ) {
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
      if (this.disposed) return;
      try {
        const response = await this.write(() => this.deps.follow(root, state));
        if (this.pendingFollows.get(root) === state) {
          this.pendingFollows.delete(root);
        }
        this.followFailures = 0;
        if (!this.disposed) {
          this.deps.onWritten?.({ type: "thread_follow", response });
        }
      } catch (error) {
        if (this.disposed) return;
        if (isChatStateAccountChanged(error)) {
          this.stopForAccountChange();
          return;
        }
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
