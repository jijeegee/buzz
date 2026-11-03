import type { RelaySubscriptionFilter } from "@/shared/api/relayClientShared";
import type { MeListenerOptions } from "@/shared/api/relayMeBundle";
import { meLive } from "@/shared/api/relayMeLive";
import type { RelayEvent } from "@/shared/api/types";

/** A failed read-state fetch or listener retries after 1 s, doubling to 60 s. */
const RETRY_BASE_MS = 1_000;
const RETRY_MAX_MS = 60_000;

export function retryDelayMs(attempt: number): number {
  return Math.min(RETRY_BASE_MS * 2 ** attempt, RETRY_MAX_MS);
}

/** Opens the live read-state listener; resolves with its stop function. */
export type ReadStateListen = (
  filter: RelaySubscriptionFilter,
  onEvent: (event: RelayEvent) => void,
  options: MeListenerOptions,
) => Promise<() => void | Promise<void>>;

/**
 * The listener a manager uses unless one is injected: the authored account
 * bundle. Tests replace `listen`, as they replace `meLive.subscribe`.
 */
export const readStateLive: { listen: ReadStateListen } = {
  listen: (filter, onEvent, options) =>
    meLive.subscribe(filter, onEvent, undefined, options),
};

/**
 * Runs a fetch one at a time: a call during a fetch runs one more after it.
 * A failure retries with backoff until a fetch succeeds, because a live-only
 * listener carries none of the marks the fetch missed.
 */
export class RetryingFetch {
  private inFlight: Promise<boolean> | null = null;
  private again = false;
  private retryTimer: number | null = null;
  private attempt = 0;
  private stopped = false;
  private readonly fetch: () => Promise<boolean>;
  private readonly onSuccess: () => void;

  constructor(fetch: () => Promise<boolean>, onSuccess: () => void) {
    this.fetch = fetch;
    this.onSuccess = onSuccess;
  }

  run(): Promise<boolean> {
    if (this.stopped) return Promise.resolve(false);
    if (this.inFlight) {
      this.again = true;
      return this.inFlight;
    }
    const flight = (async () => {
      let ok = await this.fetch();
      while (this.again && !this.stopped) {
        this.again = false;
        ok = await this.fetch();
      }
      return ok;
    })();
    this.inFlight = flight;
    void flight.then((ok) => {
      this.inFlight = null;
      if (this.stopped) return;
      if (ok) {
        this.attempt = 0;
        this.onSuccess();
      } else if (this.retryTimer === null) {
        this.retryTimer = window.setTimeout(
          () => {
            this.retryTimer = null;
            void this.run();
          },
          retryDelayMs(this.attempt++),
        );
      }
    });
    return flight;
  }

  stop(): void {
    this.stopped = true;
    if (this.retryTimer !== null) window.clearTimeout(this.retryTimer);
    this.retryTimer = null;
  }
}

/**
 * One live listener at a time. A failed attach, or a terminal CLOSED that
 * removes it (`onRemoved`), re-attaches with backoff; `onReattached` then
 * refetches what was missed meanwhile.
 */
export type ReattachingListenerOptions = {
  listen: ReadStateListen;
  filter: RelaySubscriptionFilter;
  onEvent: (event: RelayEvent) => void;
  onGap: () => void;
  onReattached: () => void;
};

export class ReattachingListener {
  state: "idle" | "attaching" | "live" = "idle";
  private stopLive: (() => void | Promise<void>) | null = null;
  private timer: number | null = null;
  private attempt = 0;
  private stopped = false;
  private readonly options: ReattachingListenerOptions;

  constructor(options: ReattachingListenerOptions) {
    this.options = options;
  }

  async attach(): Promise<boolean> {
    if (this.stopped || this.state !== "idle") return false;
    this.state = "attaching";
    try {
      const { listen, filter, onEvent, onGap } = this.options;
      const stop = await listen(filter, onEvent, {
        onGap,
        onRemoved: () => {
          if (this.stopped || this.state !== "live") return;
          this.stopLive = null;
          this.state = "idle";
          this.scheduleReattach();
        },
      });
      if (this.stopped) {
        void stop();
        return false;
      }
      this.stopLive = stop;
      this.state = "live";
      this.attempt = 0;
      return true;
    } catch (error) {
      console.debug("[ReadStateManager] live subscription FAILED:", error);
      this.state = "idle";
      this.scheduleReattach();
      return false;
    }
  }

  private scheduleReattach(): void {
    if (this.stopped || this.timer !== null) return;
    this.timer = window.setTimeout(
      () => {
        this.timer = null;
        void this.attach().then((ok) => {
          if (ok) this.options.onReattached();
        });
      },
      retryDelayMs(this.attempt++),
    );
  }

  stop(): void {
    this.stopped = true;
    if (this.timer !== null) window.clearTimeout(this.timer);
    this.timer = null;
    const stop = this.stopLive;
    this.stopLive = null;
    if (stop) void stop();
  }
}

/** Least-recently-used map from event id to its parsed blob. */
export class ParseCache<T> {
  private entries = new Map<string, T>();
  private readonly size: number;

  constructor(size: number) {
    this.size = size;
  }

  async get(id: string, parse: () => Promise<T | null>): Promise<T | null> {
    const cached = this.entries.get(id);
    if (cached !== undefined) {
      this.entries.delete(id);
      this.entries.set(id, cached);
      return cached;
    }
    const parsed = await parse();
    // A failed parse is not cached: a later delivery tries again.
    if (parsed !== null) {
      this.entries.set(id, parsed);
      if (this.entries.size > this.size) {
        const oldest = this.entries.keys().next().value;
        if (oldest !== undefined) this.entries.delete(oldest);
      }
    }
    return parsed;
  }
}
