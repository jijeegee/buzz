import { invokeTauri } from "@/shared/api/tauri";
import type { RelayEvent } from "@/shared/api/types";

export async function getEventById(eventId: string): Promise<RelayEvent> {
  const eventJson = await invokeTauri<string>("get_event", { eventId });
  return JSON.parse(eventJson) as RelayEvent;
}

export async function getEventsByIds(
  eventIds: string[],
): Promise<RelayEvent[]> {
  if (eventIds.length === 0) return [];
  return invokeTauri<RelayEvent[]>("get_events", { eventIds });
}

/**
 * Relay-signed excerpt (kind:39008) of a quoted message the reader cannot
 * fetch because it was sent before they joined, or null when the relay has
 * none. `quotingId` is the visible message carrying the quote.
 */
export async function getQuotePreview(
  channelId: string,
  quotedId: string,
  quotingId: string,
): Promise<RelayEvent | null> {
  return invokeTauri<RelayEvent | null>("get_quote_preview", {
    channelId,
    quotedId,
    quotingId,
  });
}
