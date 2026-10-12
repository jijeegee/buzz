import { invokeTauri } from "@/shared/api/tauri";
import type { ChannelPageCursor, RelayEvent } from "@/shared/api/types";

/** Fetch the flat Nostr event array for one server-assembled channel window. */
export async function getChannelWindowEvents(
  channelId: string,
  cursor: ChannelPageCursor | null = null,
  limitRows = 50,
): Promise<RelayEvent[]> {
  return invokeTauri<RelayEvent[]>("get_channel_window", {
    channelId,
    limitRows,
    cursor: cursor
      ? { created_at: cursor.createdAt, event_id: cursor.eventId }
      : null,
  });
}

/**
 * Fetch the newest window of several channels in one relay round trip, keyed
 * by channel. Channels the relay skipped (no access) are absent.
 */
export async function getChannelWindowsEvents(
  channelIds: readonly string[],
  limitRows: number,
): Promise<Map<string, RelayEvent[]>> {
  const entries = await invokeTauri<
    Array<{ channel_id: string; events: RelayEvent[] }>
  >("get_channel_windows", { channelIds, limitRows });
  return new Map(entries.map((entry) => [entry.channel_id, entry.events]));
}
