import { CHANNEL_EVENT_KINDS } from "@/shared/constants/kinds";
import {
  MAX_EXPLICIT_CHANNEL_VALUES,
  type RelaySubscription,
  type RelaySubscriptionFilter,
} from "@/shared/api/relayClientShared";
import type { RelayEvent } from "@/shared/api/types";
import {
  markReconnectRepairDone,
  shouldDispatchSubscriptionEvent,
} from "@/shared/api/relayClosedRecovery";
import type { ChannelReconnectRepairRequest } from "@/shared/api/channelReconnectRepair";
import { noteStoreRepairCompleted } from "@/shared/api/relayStoreCoverage";
import {
  isRateLimited,
  waitForRateLimit,
} from "@/shared/api/relayRateLimitGate";

const RECONNECT_REPLAY_SKEW_SECS = 5;
/**
 * Legacy cursors are event-time maxima, so cover relay future tolerance (900s)
 * plus the channel-row storage fence and margin (960s + 5s).
 */
export const RELAY_INGEST_FUTURE_TOLERANCE_SECS = 900;
export const DB_CREATED_AT_FLOOR_SECS = 960;
export const FENCE_CLOCK_MARGIN_SECS = 5;
export const RECONNECT_REPLAY_CHANNEL_LOOKBACK_SECS =
  RELAY_INGEST_FUTURE_TOLERANCE_SECS +
  DB_CREATED_AT_FLOOR_SECS +
  FENCE_CLOCK_MARGIN_SECS;
export const RECONNECT_REPLAY_PAGE_LIMIT = 500;
export const RECONNECT_REPLAY_PAGE_CONCURRENCY = 4;

/**
 * Maximum attempts for one subscription's paged history backfill.
 *
 * Backfill failures must never escape `replayLiveSubscriptions`: by the time
 * paging starts, every live REQ has already been re-established on a healthy,
 * authenticated socket. Letting a history rejection propagate makes the
 * session tear that socket down (`resetConnection`) and reconnect straight
 * into the same rate-limit window — the "briefly connected → can't reach the
 * relay" flap loop. Instead each sub retries behind the rate-limit gate a
 * bounded number of times, then degrades to live-only for this connection.
 * The window's lower bound is pinned in `pendingReplaySince` while unresolved
 * (live events advance `lastSeenCreatedAt` regardless of backfill success),
 * so the next reconnect still requests the missed window.
 */
export const PAGE_REPLAY_MAX_ATTEMPTS = 3;

/**
 * Maximum live subscriptions sent per relay REQ burst during reconnect.
 *
 * Capping the initial blast prevents admission-control bursts on degraded
 * networks where the relay is already near its per-pubkey quota.
 */
export const REPLAY_BATCH_SIZE = 8;

/**
 * Delay between consecutive replay batches (milliseconds).
 *
 * Spreads the REQ storm across time so the relay's sliding quota window
 * can absorb each batch without triggering rate-limiting on the next.
 */
export const REPLAY_INTER_BATCH_DELAY_MS = 50;

async function runWithConcurrency<T>(
  items: T[],
  concurrency: number,
  worker: (item: T) => Promise<void>,
) {
  const workerCount = Math.min(Math.max(1, concurrency), items.length);
  let nextIndex = 0;

  await Promise.all(
    Array.from({ length: workerCount }, async () => {
      while (nextIndex < items.length) {
        const item = items[nextIndex++];
        await worker(item);
      }
    }),
  );
}

export function buildReconnectReplayFilter(
  filter: RelaySubscriptionFilter,
  since?: number,
  until?: number,
  limit = Math.min(filter.limit, RECONNECT_REPLAY_PAGE_LIMIT),
) {
  if (since === undefined) return filter;

  const replayFilter: RelaySubscriptionFilter = {
    ...filter,
    limit,
    since: filter.since === undefined ? since : Math.max(filter.since, since),
  };

  if (until !== undefined) {
    replayFilter.until =
      filter.until === undefined ? until : Math.min(filter.until, until);
  }

  return replayFilter;
}

/**
 * Reconnect cursors for a new live subscription. A repairable one is
 * continuous from now on, and its repair never surfaces rows from before its
 * rooms went live as new (see `liveSince`). A room another live subscription
 * already carries (a chunk being replaced, maybe while offline) keeps that
 * subscription's start, so the replacement still surfaces what it missed.
 */
export function reconnectCursors(
  filter: RelaySubscriptionFilter,
  subscriptions: Iterable<RelaySubscription>,
) {
  if (!shouldPageReconnectReplay(filter)) return {};
  const now = Math.floor(Date.now() / 1_000);
  const liveSince: Record<string, number> = {};
  for (const roomId of filter["#h"] ?? []) liveSince[roomId] = now;
  for (const other of subscriptions) {
    if (other.mode !== "live" || !other.liveSince) continue;
    for (const [roomId, since] of Object.entries(other.liveSince)) {
      if (liveSince[roomId] !== undefined && since < liveSince[roomId]) {
        liveSince[roomId] = since;
      }
    }
  }
  return { lastSeenCreatedAt: now, liveSince };
}

/**
 * Whether a live subscription's missed window is repaired over HTTP: it must
 * carry every channel event kind for an explicit set of rooms (one room, or a
 * room bundle of up to the relay's 128-channel cap). Room bundles are
 * live-only (`limit: 0`), so their repair is the only replay they get.
 */
export function shouldPageReconnectReplay(filter: RelaySubscriptionFilter) {
  const roomIds = filter["#h"];
  return (
    Array.isArray(roomIds) &&
    roomIds.length > 0 &&
    roomIds.length <= MAX_EXPLICIT_CHANNEL_VALUES &&
    CHANNEL_EVENT_KINDS.every((kind) => filter.kinds.includes(kind))
  );
}

/** Rows older than this were never live on `subscription`'s room. */
function liveFloor(
  subscription: Extract<RelaySubscription, { mode: "live" }>,
  event: RelayEvent,
) {
  const starts = subscription.liveSince;
  if (!starts) return Number.NEGATIVE_INFINITY;
  const roomId = event.tags.find((tag) => tag[0] === "h")?.[1];
  const since =
    (roomId === undefined ? undefined : starts[roomId]) ??
    Math.min(...Object.values(starts));
  return since - RECONNECT_REPLAY_SKEW_SECS;
}

/**
 * Page one subscription's missed-window history.
 *
 * Returns `true` only when the window was genuinely completed (short page or
 * boundary reached). Returns `false` when the pass aborted because the
 * connection went stale (`isActive()` false) — callers must NOT treat that as
 * completion: the same subscription object is shared with the superseding
 * connection, and clearing its pinned `pendingReplaySince` on a stale abort
 * would erase the floor the new connection still needs.
 */
export async function replayReconnectHistoryPages({
  subscription,
  channelIds,
  since,
  until,
  isActive,
  requestRepair,
}: {
  subscription: Extract<RelaySubscription, { mode: "live" }>;
  channelIds: string[];
  since: number;
  until?: number;
  isActive: () => boolean;
  requestRepair: (
    request: ChannelReconnectRepairRequest,
  ) => Promise<RelayEvent[]>;
}): Promise<boolean> {
  let pageUntil: number | undefined = until;
  let beforeId: string | undefined;

  while (pageUntil === undefined || pageUntil >= since) {
    if (!isActive()) return false;

    const events = await requestRepair({
      channelIds,
      since,
      limit: RECONNECT_REPLAY_PAGE_LIMIT,
      until: pageUntil,
      beforeId,
    });

    if (!isActive()) return false;

    // The walk starts before `liveSince` so the device store gets every row;
    // older rows were never live on this subscription and must not surface
    // as new activity.
    for (const event of events) {
      if (event.created_at < liveFloor(subscription, event)) continue;
      if (shouldDispatchSubscriptionEvent(subscription, event)) {
        subscription.onEvent(event);
      }
    }
    if (events.length < RECONNECT_REPLAY_PAGE_LIMIT) return true;

    const terminal = events.at(-1);
    if (!terminal || terminal.created_at < since) return true;
    if (terminal.created_at === pageUntil && terminal.id === beforeId) {
      throw new Error("Reconnect repair cursor did not advance");
    }
    if (terminal.created_at === since) {
      // Continue across an arbitrarily dense boundary second until the relay
      // returns a short page; the composite cursor, not timestamp subtraction,
      // guarantees progress.
      pageUntil = since;
    } else {
      pageUntil = terminal.created_at;
    }
    beforeId = terminal.id;
  }
  return true;
}

export async function replayLiveSubscriptions({
  subscriptions,
  sendRaw,
  requestRepair,
  generation = 0,
  pageReplayConcurrency = RECONNECT_REPLAY_PAGE_CONCURRENCY,
  visibleChannelId = null,
  replayBatchSize = REPLAY_BATCH_SIZE,
  interBatchDelayMs = REPLAY_INTER_BATCH_DELAY_MS,
  setTimeoutFn = (fn: () => void, ms: number) =>
    window.setTimeout(fn, ms) as unknown as number,
  isActive = () => true,
}: {
  subscriptions: Map<string, RelaySubscription>;
  sendRaw: (payload: unknown[]) => Promise<void>;
  requestRepair: (
    request: ChannelReconnectRepairRequest,
  ) => Promise<RelayEvent[]>;
  generation?: number;
  pageReplayConcurrency?: number;
  /** Channel currently visible in the UI — its subscriptions go in the first batch. */
  visibleChannelId?: string | null;
  /** Max subscriptions per REQ burst (injectable for tests). */
  replayBatchSize?: number;
  /** Milliseconds between bursts (injectable for tests). */
  interBatchDelayMs?: number;
  /** setTimeout implementation (injectable for tests). */
  setTimeoutFn?: (fn: () => void, ms: number) => number;
  /**
   * Returns false when the connection that initiated this replay has been
   * superseded by a newer one. After the gate await resumes, a stale replay
   * must not double-send REQs on the live socket.
   */
  isActive?: () => boolean;
}) {
  // If the relay has signalled back-pressure, wait for the gate to clear
  // before blasting a full set of REQs that would immediately be rate-limited.
  if (isRateLimited()) await waitForRateLimit();

  // A newer connection may have replayed while this one was suspended at the
  // gate — abort silently to avoid double-sending every REQ on the live socket.
  if (!isActive()) return;

  const replayRequests = Array.from(subscriptions.entries())
    .filter(
      (
        entry,
      ): entry is [string, Extract<RelaySubscription, { mode: "live" }>] =>
        entry[1].mode === "live",
    )
    .map(([subId, subscription]) => {
      const channelIds = subscription.filter["#h"];
      const shouldPageReplay = shouldPageReconnectReplay(subscription.filter);
      const cursorSince =
        subscription.lastSeenCreatedAt === undefined
          ? undefined
          : Math.max(
              0,
              subscription.lastSeenCreatedAt -
                (shouldPageReplay
                  ? RECONNECT_REPLAY_CHANNEL_LOOKBACK_SECS
                  : RECONNECT_REPLAY_SKEW_SECS),
            );
      // A pinned floor from a previously failed backfill takes precedence
      // over the cursor: live events kept advancing `lastSeenCreatedAt`
      // while the older window stayed unresolved, and starting from the
      // cursor would skip it permanently.
      const replayFloor =
        cursorSince === undefined
          ? subscription.pendingReplaySince
          : Math.min(cursorSince, subscription.pendingReplaySince ?? Infinity);
      // Native repair bypasses the original WS filter, so preserve its lower
      // bound explicitly. Otherwise a from-now subscription can replay older
      // rows and surface stale unread or notification activity on reconnect.
      const replaySince =
        replayFloor === undefined
          ? undefined
          : Math.max(replayFloor, subscription.filter.since ?? 0);
      const willRepair = shouldPageReplay && replaySince !== undefined;
      if (willRepair) {
        // Install before the restored live REQ: a live frame may beat page one.
        subscription.reconnectReplay = {
          generation,
          seenEventIds: new Set(),
          liveEose: false,
          repairDone: false,
        };
      }

      return {
        subId,
        subscription,
        channelIds,
        replaySince,
        shouldPageReplay: willRepair,
      };
    });

  // Visible timeline and interactive consumers (including off-screen huddle
  // speech) recover before cold work. Stable sort preserves registration order
  // within each tier, including ties between visible and interactive owners.
  const isForeground = (
    subscription: Extract<RelaySubscription, { mode: "live" }>,
  ) =>
    subscription.priority === "interactive" ||
    (visibleChannelId !== null &&
      (subscription.filter["#h"]?.includes(visibleChannelId) ?? false));
  replayRequests.sort(
    (a, b) =>
      Number(isForeground(b.subscription)) -
      Number(isForeground(a.subscription)),
  );

  // Send live REQs in capped batches with inter-batch delays to avoid
  // triggering per-pubkey admission control on degraded/recovering connections.
  for (let i = 0; i < replayRequests.length; i += replayBatchSize) {
    // Re-check the gate before every batch: a previous batch may have triggered
    // admission control and armed the gate mid-replay. Wait for it to clear,
    // then verify the connection is still current — a newer connection may have
    // replayed while we were suspended.
    if (isRateLimited()) await waitForRateLimit();
    if (!isActive()) return;
    const batch = replayRequests.slice(i, i + replayBatchSize);
    await Promise.all(
      batch.map(
        async ({ subId, subscription, replaySince, shouldPageReplay }) => {
          // The subscription map may change while replay is paused behind the
          // gate or an inter-batch delay. Never resurrect a disposed entry (or
          // overwrite its replacement) with a REQ from this stale snapshot.
          if (!isActive() || subscriptions.get(subId) !== subscription) return;
          await sendRaw([
            "REQ",
            subId,
            shouldPageReplay
              ? subscription.filter
              : buildReconnectReplayFilter(subscription.filter, replaySince),
          ]);
          // Disposal can race the asynchronous websocket invoke after the
          // identity check. If this exact entry was deleted while REQ was in
          // flight, CLOSE again after REQ settles so wire order converges to
          // closed. A different object under the same ID is a replacement and
          // must not be torn down by this stale replay.
          if (isActive() && !subscriptions.has(subId)) {
            await sendRaw(["CLOSE", subId]);
          }
        },
      ),
    );
    if (i + replayBatchSize < replayRequests.length) {
      await new Promise<void>((resolve) =>
        setTimeoutFn(resolve, interBatchDelayMs),
      );
    }
  }

  await runWithConcurrency(
    replayRequests.filter(
      (
        request,
      ): request is typeof request & {
        channelIds: string[];
        replaySince: number;
        shouldPageReplay: true;
      } =>
        request.shouldPageReplay &&
        request.channelIds !== undefined &&
        request.replaySince !== undefined,
    ),
    pageReplayConcurrency,
    async ({ subId, subscription, channelIds, replaySince }) => {
      // Backfill is best-effort: a failure here (typically a `rate-limited:`
      // CLOSED on a history REQ) must never escape to the session and tear
      // down the healthy, authenticated socket carrying the live REQs — that
      // is the connect→drop flap loop. Retry behind the gate a bounded number
      // of times, then degrade to live-only for this connection.
      //
      // Pin the window's lower bound before the first attempt: events on the
      // already-restored live REQ advance `lastSeenCreatedAt` independently
      // of backfill success, so without the pin an exhausted backfill
      // followed by one live event would make the next reconnect skip the
      // unresolved window permanently. Cleared only on a completed pass.
      subscription.pendingReplaySince = replaySince;
      for (let attempt = 1; attempt <= PAGE_REPLAY_MAX_ATTEMPTS; attempt++) {
        try {
          const completed = await replayReconnectHistoryPages({
            subscription,
            channelIds,
            since: replaySince,
            // Do not trust the renderer clock as the upper bound. The relay's
            // newest matching row starts the keyset walk.
            until: undefined,
            // Both guards are required. The identity check catches the sub
            // being torn down/replaced; the outer isActive() catches
            // connection supersession, which bumps the generation while the
            // SAME subscription key and object survive in the map — identity
            // alone stays true and a stale pass could complete and clear the
            // floor the superseding connection needs.
            isActive: () =>
              isActive() && subscriptions.get(subId) === subscription,
            requestRepair,
          });
          // A stale-connection abort is NOT completion: the superseding
          // connection shares this subscription object and still needs the
          // pinned floor for its own replay. Only a genuinely completed
          // window may release it.
          if (completed) {
            subscription.pendingReplaySince = undefined;
            noteStoreRepairCompleted(subscription, replaySince);
            markReconnectRepairDone(subscription, generation);
          }
          return;
        } catch (error) {
          console.warn(
            `[reconnect replay] history backfill attempt ${attempt}/${PAGE_REPLAY_MAX_ATTEMPTS} failed for ${subId}:`,
            error,
          );
          if (attempt === PAGE_REPLAY_MAX_ATTEMPTS) {
            markReconnectRepairDone(subscription, generation);
            return;
          }
          // The failed REQ's CLOSED handler arms the rate-limit gate before
          // rejecting; wait for it (no-op when the failure wasn't back-pressure)
          // and re-check that this replay's connection is still current.
          if (isRateLimited()) await waitForRateLimit();
          if (subscriptions.get(subId) !== subscription || !isActive()) return;
        }
      }
    },
  );
}
