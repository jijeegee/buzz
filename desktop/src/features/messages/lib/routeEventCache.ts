import type { QueryClient } from "@tanstack/react-query";

import type { RelayEvent } from "@/shared/api/types";

const MAX_ROUTE_EVENTS = 300;
/** Older copies go back to the relay, so later edits and deletions apply. */
const ROUTE_EVENT_TTL_MS = 5 * 60 * 1_000;
type RememberedEvent = { event: RelayEvent; at: number };
const routeEvents = new WeakMap<QueryClient, Map<string, RememberedEvent>>();

/**
 * Remember events a message link may open on (a thread reply and its root),
 * loaded in the background, so following the link need not ask the relay.
 * Scoped to the query client like the rest of the message cache.
 */
export function rememberRouteEvents(
  queryClient: QueryClient,
  events: readonly RelayEvent[],
): void {
  let remembered = routeEvents.get(queryClient);
  if (!remembered) {
    remembered = new Map();
    routeEvents.set(queryClient, remembered);
  }
  const at = Date.now();
  for (const event of events) {
    remembered.delete(event.id);
    remembered.set(event.id, { event, at });
  }
  for (const id of remembered.keys()) {
    if (remembered.size <= MAX_ROUTE_EVENTS) break;
    remembered.delete(id);
  }
}

export function getRememberedRouteEvent(
  queryClient: QueryClient,
  eventId: string,
): RelayEvent | null {
  const entry = routeEvents.get(queryClient)?.get(eventId);
  if (!entry || Date.now() - entry.at > ROUTE_EVENT_TTL_MS) return null;
  return entry.event;
}
