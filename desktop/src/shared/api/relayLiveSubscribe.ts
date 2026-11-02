import {
  type LiveMultiFilter,
  type LiveSubscriptionReadiness,
  liveReqFrame,
  type RelaySubscription,
  type RelaySubscriptionFilter,
} from "@/shared/api/relayClientShared";
import { retireLiveSubscription } from "@/shared/api/relayClosedRecovery";
import { reconnectCursors } from "@/shared/api/relayReconnectReplay";
import type { RelayLiveReqDrain } from "@/shared/api/relayLiveReqDrain";
import type { RelayEvent } from "@/shared/api/types";
import { namesUserStream } from "@/shared/api/userStreamRules";

/**
 * What a live REQ needs from the session (`RelayClient`). Every member is
 * read at call time: tests swap the socket id, generation and connect.
 */
export type LiveSessionHost = {
  readonly subscriptions: Map<string, RelaySubscription>;
  readonly liveReqDrain: RelayLiveReqDrain;
  sessionEpoch(): number;
  sessionSignal(): AbortSignal;
  connectionGeneration(): number;
  visibleChannelId(): string | null;
  ensureConnected(): Promise<unknown>;
  sendRawForGeneration(payload: unknown[], generation: number): Promise<void>;
  recoverFromSocketFailure(error: unknown, fallbackMessage: string): Error;
  closeSubscription(subId: string): Promise<void>;
};

/**
 * Send a REQ for `payload[1]`, retrying once on a fresh socket. Live REQs
 * queue in the drain (visible room and interactive first, then `limit: 0`,
 * then history); a REQ whose entry was disposed meanwhile is never sent, and
 * one disposed while it was being sent is closed again on that socket.
 */
export async function sendRawWithReconnectRetry(
  host: LiveSessionHost,
  payload: unknown[],
  fallbackMessage: string,
  onDispatch?: () => void,
) {
  const epoch = host.sessionEpoch();
  const subId = payload[1] as string;
  const subscription = host.subscriptions.get(subId);
  const isOwned = () =>
    subscription !== undefined &&
    epoch === host.sessionEpoch() &&
    host.subscriptions.get(subId) === subscription;
  const checkOwned = () => {
    if (!isOwned())
      throw new DOMException("Relay subscription retired.", "AbortError");
  };
  let generation = host.connectionGeneration();
  const send = async () => {
    checkOwned();
    generation = host.connectionGeneration();
    try {
      onDispatch?.();
      await host.sendRawForGeneration(payload, generation);
    } finally {
      // A dispatched REQ cannot be unsent. Converge to CLOSE on that socket,
      // without touching a replacement workspace or connection.
      if (
        !isOwned() &&
        epoch === host.sessionEpoch() &&
        generation === host.connectionGeneration()
      ) {
        await host.closeSubscription(subId).catch(() => {});
      }
    }
  };
  const dispatch = () => {
    const queuedGeneration = host.connectionGeneration();
    return subscription?.mode === "live"
      ? host.liveReqDrain.run(
          subId,
          () => isOwned() && queuedGeneration === host.connectionGeneration(),
          () => {
            const visible = host.visibleChannelId();
            return subscription.priority === "interactive" ||
              (visible !== null &&
                subscription.filter["#h"]?.includes(visible) === true)
              ? 0
              : subscription.filter.limit === 0
                ? 1
                : 2;
          },
          send,
        )
      : send();
  };
  try {
    await dispatch();
  } catch (error) {
    checkOwned(); // Cancellation is not a socket failure and must not reconnect.
    if (generation !== host.connectionGeneration()) {
      // Live entries survive reset and belong to the new connection's replay.
      // A late old-socket failure must not tear down that retained ownership.
      if (subscription?.mode === "live") return;
      throw error;
    }
    const normalizedError = host.recoverFromSocketFailure(
      error,
      fallbackMessage,
    );
    let retryGeneration: number | null = null;
    try {
      await host.ensureConnected();
      checkOwned();
      retryGeneration = host.connectionGeneration();
      await dispatch();
    } catch (retryError) {
      checkOwned();
      if (generation !== host.connectionGeneration()) {
        if (retryGeneration !== null && subscription?.mode === "live") return;
        throw retryError;
      }
      throw host.recoverFromSocketFailure(retryError, normalizedError.message);
    }
  }
}

/**
 * Open a live REQ. Resolves with its dispose function once the relay sent
 * EOSE (or the readiness fallback fired after dispatch); rejects, leaving
 * nothing registered, when it fails or `signal` aborts first.
 */
export async function subscribeLiveSession(
  host: LiveSessionHost,
  sendReq: (
    payload: unknown[],
    fallbackMessage: string,
    onDispatch?: () => void,
  ) => Promise<void>,
  filter: RelaySubscriptionFilter,
  onEvent: (event: RelayEvent) => void,
  onReady?: (readiness: LiveSubscriptionReadiness) => void,
  readinessTimeoutMs = 250,
  signal?: AbortSignal,
  onGone?: () => void,
  priority?: "interactive",
  multi?: LiveMultiFilter,
) {
  // `$joined` is the user stream's own REQ, never a room of a live REQ.
  if (namesUserStream(filter) || multi?.filters.some(namesUserStream)) {
    throw new Error("The user stream is not a live room subscription.");
  }
  const epoch = host.sessionEpoch();
  const sessionSignal = host.sessionSignal();
  signal?.throwIfAborted();
  const subId = `live-${crypto.randomUUID()}`;
  let fallbackTimeout: number | undefined;
  let settleReady = () => {};
  const onRemoved = () => {
    signal?.removeEventListener("abort", abort);
    sessionSignal.removeEventListener("abort", abort);
    window.clearTimeout(fallbackTimeout);
    settleReady();
    onGone?.();
  };
  const subscription: Extract<RelaySubscription, { mode: "live" }> = {
    mode: "live",
    filter,
    multi,
    priority,
    onEvent,
    onRemoved,
    ...reconnectCursors(filter, host.subscriptions.values()),
  };
  const dispose = async () => {
    if (host.subscriptions.get(subId) !== subscription) return;
    host.subscriptions.delete(subId);
    host.liveReqDrain.cancel(subId);
    retireLiveSubscription(subscription);
    onRemoved();
    // Workspace teardown closes its socket; never send on its replacement.
    if (epoch === host.sessionEpoch()) await host.closeSubscription(subId);
  };
  let rejectCancelled = (_error: Error) => {};
  const cancelled = new Promise<never>((_resolve, reject) => {
    rejectCancelled = reject;
  });
  const abort = () => {
    rejectCancelled(
      new DOMException("Live subscription cancelled.", "AbortError"),
    );
    void dispose().catch(() => {});
    onRemoved();
  };
  signal?.addEventListener("abort", abort, { once: true });
  sessionSignal.addEventListener("abort", abort, { once: true });
  try {
    await Promise.race([host.ensureConnected(), cancelled]);
    signal?.throwIfAborted();
    sessionSignal.throwIfAborted();
    const ready = new Promise<void>((resolve) => {
      settleReady = resolve;
    });
    let readySettled = false;
    subscription.resolveReady = (readiness) => {
      if (readySettled) return;
      readySettled = true;
      window.clearTimeout(fallbackTimeout);
      onReady?.(readiness);
      settleReady();
    };
    host.subscriptions.set(subId, subscription);
    await Promise.race([
      sendReq(
        liveReqFrame(subId, subscription, filter),
        "Failed to restore relay subscription.",
        () => {
          if (readySettled) return;
          window.clearTimeout(fallbackTimeout);
          fallbackTimeout = window.setTimeout(
            () => subscription.resolveReady?.("timeout"),
            readinessTimeoutMs,
          );
        },
      ),
      cancelled,
    ]);
    await Promise.race([ready, cancelled]);
    return dispose;
  } catch (error) {
    await dispose().catch(() => {});
    onRemoved();
    throw error;
  }
}
