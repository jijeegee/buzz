import type { RelayEvent } from "@/shared/api/types";
import { type Filter, matchFilter } from "nostr-tools/filter";

/**
 * Whether a relay-wide event (`emitMockGlobalEvent`) reaches a live REQ:
 * some filter matches it, as on the relay. Room filters (`#h`) keep the
 * mock's kinds-only rule, because room fixtures emit without `h` tags.
 */
export function mockLiveFiltersMatch(
  filters: readonly unknown[],
  event: RelayEvent,
): boolean {
  return filters.some((raw) => {
    const filter = raw as Filter;
    if (filter.kinds && !filter.kinds.includes(event.kind)) return false;
    if (filter["#h"]) return true;
    const { limit: _limit, since: _since, until: _until, ...rest } = filter;
    return matchFilter(rest, event);
  });
}

/**
 * Answer a one-shot REQ with several filters through a single-filter
 * answerer: events once each across filters, one EOSE after every filter's
 * EOSE, or the first CLOSED instead.
 */
export function answerEachFilter<F>(
  filters: readonly F[],
  send: (frame: unknown[]) => void,
  answer: (filter: F, send: (frame: unknown[]) => void) => void,
) {
  const sent = new Set<string>();
  let pending = filters.length;
  let done = false;
  const relay = (frame: unknown[]) => {
    if (done) return;
    if (frame[0] === "EVENT") {
      const id = (frame[2] as RelayEvent).id;
      if (sent.has(id)) return;
      sent.add(id);
    } else if (frame[0] === "EOSE") {
      pending -= 1;
      if (pending > 0) return;
      done = true;
    } else if (frame[0] === "CLOSED") {
      done = true;
    }
    send(frame);
  };
  for (const filter of filters) answer(filter, relay);
}
