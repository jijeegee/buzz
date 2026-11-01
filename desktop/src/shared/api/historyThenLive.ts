import type { RelayEvent } from "@/shared/api/types";

const SEEN_LIMIT = 2_000;

export type HistoryThenLiveOptions = {
  /** Start listening; resolves once live with the stop function. */
  listen: (apply: (event: RelayEvent) => void) => Promise<() => Promise<void>>;
  /** One history fetch; it runs only after `listen` is live. */
  fetch: () => Promise<RelayEvent[]>;
  /** Called once per event id, from either source. */
  apply: (event: RelayEvent) => void;
  /** A fetch failed; it is never treated as an empty result. */
  onFetchError?: (error: unknown) => void;
};

export type HistoryThenLive = {
  /** Whether the first fetch succeeded (false after a failure or `stop`). */
  firstFetch: Promise<boolean>;
  /** Fetch again through the same gate; resolves whether it succeeded. */
  refetch: () => Promise<boolean>;
  stop: () => Promise<void>;
};

/**
 * Listen first, then fetch history once, so nothing published in between is
 * lost (AGENTS.md, rule 2). Live and fetched events share one `seen` gate, so
 * an event arriving from both sources is applied once; fetched events are
 * applied oldest first. A `stop` during a fetch drops its result. Rejects,
 * with nothing registered, if `listen` fails.
 */
export async function historyThenLive(
  options: HistoryThenLiveOptions,
): Promise<HistoryThenLive> {
  const seen = new Map<string, true>();
  let stopped = false;
  const gate = (event: RelayEvent) => {
    if (stopped || seen.has(event.id)) return;
    seen.set(event.id, true);
    if (seen.size > SEEN_LIMIT) {
      const oldest = seen.keys().next().value;
      if (oldest !== undefined) seen.delete(oldest);
    }
    options.apply(event);
  };

  const stopLive = await options.listen(gate);

  const refetch = async () => {
    if (stopped) return false;
    let events: RelayEvent[];
    try {
      events = await options.fetch();
    } catch (error) {
      if (!stopped) options.onFetchError?.(error);
      return false;
    }
    if (stopped) return false;
    for (const event of [...events].sort(
      (a, b) => a.created_at - b.created_at,
    )) {
      gate(event);
    }
    return true;
  };

  return {
    firstFetch: refetch(),
    refetch,
    stop: async () => {
      if (stopped) return;
      stopped = true;
      await stopLive();
    },
  };
}
