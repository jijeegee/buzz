import type {
  LiveMultiFilter,
  LiveSubscriptionReadiness,
  RelaySubscription,
  RelaySubscriptionFilter,
} from "@/shared/api/relayClientShared";
import { liveReqFrame } from "@/shared/api/relayClientShared";
import {
  type LiveSessionHost,
  subscribeLiveSession,
} from "@/shared/api/relayLiveSubscribe";
import { rateLimitRemainingMs } from "@/shared/api/relayRateLimitGate";
import type { RelayEvent } from "@/shared/api/types";

/** The relay's filters-per-REQ limit. */
export const MAX_LIVE_FILTERS = 10;

/** A bundle-owned live REQ with several filters (see `subscribeLiveFilters`). */
export type LiveFiltersHandle = {
  /** Close the REQ; pending `setFilters` calls resolve. */
  close: () => Promise<void>;
  /**
   * Replace the filters under the same subId. Resolves once the relay
   * answered (EOSE or CLOSED) the REQ that carries this set, or the REQ is
   * gone.
   */
  setFilters: (filters: RelaySubscriptionFilter[]) => Promise<void>;
};

/**
 * Bundle filters are live only: `limit: 0`, no `since`, no room (`#h`) and
 * no search, at most ten. Reconnect replay re-sends them unchanged.
 */
export function assertBundleLiveFilters(filters: RelaySubscriptionFilter[]) {
  if (filters.length === 0 || filters.length > MAX_LIVE_FILTERS) {
    throw new Error(`A live bundle REQ needs 1-${MAX_LIVE_FILTERS} filters.`);
  }
  for (const filter of filters) {
    if (
      filter.limit !== 0 ||
      filter.since !== undefined ||
      filter.until !== undefined ||
      filter["#h"] !== undefined ||
      "search" in filter
    ) {
      throw new Error("A live bundle filter must be limit 0 without history.");
    }
  }
}

function sameFilters(
  left: RelaySubscriptionFilter[],
  right: RelaySubscriptionFilter[],
) {
  return JSON.stringify(left) === JSON.stringify(right);
}

/**
 * Open one live REQ with several filters. At most one REQ is in flight for
 * it: the opening REQ, a reconnect replay or a CLOSED retry counts until its
 * EOSE. `setFilters` meanwhile only updates the entry, and the newest set goes
 * out after that EOSE (the relay may apply two REQs on one subId out of
 * order). Replay and CLOSED retry always carry the newest set.
 *
 * A `setFilters` REQ skips the live drain: an EVENT on this subId would
 * cancel it there, and the bundle sends it at most once per 30 s.
 */
export async function subscribeLiveFilters(
  host: LiveSessionHost,
  sendReq: (
    payload: unknown[],
    fallbackMessage: string,
    onDispatch?: () => void,
  ) => Promise<void>,
  filters: RelaySubscriptionFilter[],
  onEvent: (event: RelayEvent) => void,
  onReady?: (readiness: LiveSubscriptionReadiness) => void,
  signal?: AbortSignal,
  onGone?: () => void,
): Promise<LiveFiltersHandle> {
  assertBundleLiveFilters(filters);
  let inFlight = false;
  let dirty = false;
  let closed = false;
  let rateTimer: number | undefined;
  /** Waiting for the REQ in flight. */
  let sentWaiters: (() => void)[] = [];
  /** Waiting for a set not sent yet. */
  let heldWaiters: (() => void)[] = [];
  const releaseAll = () => {
    closed = true;
    window.clearTimeout(rateTimer);
    const waiters = [...sentWaiters, ...heldWaiters];
    sentWaiters = [];
    heldWaiters = [];
    for (const resolve of waiters) resolve();
  };
  const multi: LiveMultiFilter = {
    filters: filters.map((filter) => ({ ...filter })),
    sent() {
      inFlight = true;
      dirty = false;
      sentWaiters.push(...heldWaiters);
      heldWaiters = [];
    },
    settled() {
      inFlight = false;
      const waiters = sentWaiters;
      sentWaiters = [];
      for (const resolve of waiters) resolve();
      if (dirty) sendNow();
    },
    lost() {
      // The replay on the next socket is in flight from now on and carries
      // the newest set.
      inFlight = true;
      window.clearTimeout(rateTimer);
      rateTimer = undefined;
      heldWaiters = [...sentWaiters, ...heldWaiters];
      sentWaiters = [];
    },
  };
  const ownEntry = ():
    | [string, Extract<RelaySubscription, { mode: "live" }>]
    | null => {
    for (const [subId, subscription] of host.subscriptions) {
      if (subscription.mode === "live" && subscription.multi === multi) {
        return [subId, subscription];
      }
    }
    return null;
  };
  const sendNow = () => {
    if (closed || inFlight || rateTimer !== undefined) return;
    const own = ownEntry();
    if (!own) return;
    const waitMs = rateLimitRemainingMs();
    if (waitMs > 0) {
      rateTimer = window.setTimeout(() => {
        rateTimer = undefined;
        sendNow();
      }, waitMs);
      return;
    }
    const [subId, subscription] = own;
    const generation = host.connectionGeneration();
    const frame = liveReqFrame(subId, subscription, subscription.filter);
    void host.sendRawForGeneration(frame, generation).catch((error) => {
      // The socket is gone; its reset hands the newest set to the replay.
      if (generation === host.connectionGeneration()) {
        host.recoverFromSocketFailure(error, "Failed to update a bundle REQ.");
      }
    });
  };

  const dispose = await subscribeLiveSession(
    host,
    sendReq,
    multi.filters[0],
    onEvent,
    onReady,
    250,
    signal,
    () => {
      releaseAll();
      onGone?.();
    },
    undefined,
    multi,
  );
  return {
    close: async () => {
      releaseAll();
      await dispose();
    },
    setFilters: (next) => {
      assertBundleLiveFilters(next);
      if (closed || sameFilters(multi.filters, next)) return Promise.resolve();
      multi.filters = next.map((filter) => ({ ...filter }));
      const own = ownEntry();
      if (own) own[1].filter = multi.filters[0];
      dirty = true;
      return new Promise<void>((resolve) => {
        heldWaiters.push(resolve);
        sendNow();
      });
    },
  };
}
