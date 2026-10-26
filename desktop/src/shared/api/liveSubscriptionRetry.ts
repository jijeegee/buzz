/** Attempts before a failing live subscription waits for the next reconnect. */
export const LIVE_SUBSCRIPTION_MAX_ATTEMPTS = 6;

/** Backoff before the next attempt: 1 s doubling, at most 30 s. */
export const liveSubscriptionRetryDelayMs = (failures: number) =>
  Math.min(1_000 * 2 ** (failures - 1), 30_000);

type KeepLiveSubscriptionOptions = {
  /** Opens the subscription; resolves to its dispose. */
  open: () => Promise<() => Promise<void>>;
  /** Registers a reconnect listener; returns its unsubscribe. */
  onReconnect: (listener: () => void) => () => void;
  /** Each time a subscription is open. */
  onOpened?: () => void;
  /** Each failed attempt (cancellations are not failures). */
  onFailed?: (error: unknown) => void;
  timers?: Pick<Window, "setTimeout" | "clearTimeout">;
};

/**
 * Keep one live subscription open for as long as its owner wants it. A failed
 * open is retried with backoff; after LIVE_SUBSCRIPTION_MAX_ATTEMPTS it stops
 * and tries again on the next reconnect, so a lasting failure never loops.
 * Returns the owner's dispose, which also cancels a pending retry.
 */
export function keepLiveSubscription({
  open,
  onReconnect,
  onOpened,
  onFailed,
  timers = window,
}: KeepLiveSubscriptionOptions): () => void {
  let disposed = false;
  let opening = false;
  let close: (() => Promise<void>) | undefined;
  let retryTimer: number | undefined;
  let failures = 0;

  const attempt = () => {
    if (disposed || opening || close) return;
    opening = true;
    open().then(
      (dispose) => {
        opening = false;
        if (disposed) {
          void dispose();
          return;
        }
        close = dispose;
        failures = 0;
        onOpened?.();
      },
      (error: unknown) => {
        opening = false;
        if (disposed) return;
        if (error instanceof DOMException && error.name === "AbortError") {
          return;
        }
        failures += 1;
        onFailed?.(error);
        // Out of attempts: wait for the reconnect listener below.
        if (failures >= LIVE_SUBSCRIPTION_MAX_ATTEMPTS) return;
        retryTimer = timers.setTimeout(() => {
          retryTimer = undefined;
          attempt();
        }, liveSubscriptionRetryDelayMs(failures));
      },
    );
  };

  const stopReconnect = onReconnect(() => {
    if (close || opening || retryTimer !== undefined) return;
    failures = 0;
    attempt();
  });
  attempt();

  return () => {
    disposed = true;
    stopReconnect();
    if (retryTimer !== undefined) timers.clearTimeout(retryTimer);
    if (close) void close();
  };
}
