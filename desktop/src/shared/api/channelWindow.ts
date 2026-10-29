import {
  recordsStoreHead,
  requestWithStoreHeads,
  roomWindowAnswer,
} from "@/shared/api/relayStoreHeads";
import { invokeTauri } from "@/shared/api/tauri";
import type { ChannelPageCursor, RelayEvent } from "@/shared/api/types";

/**
 * Fetch the flat Nostr event array for one server-assembled channel window.
 * A newest window asked with `storeHead` (the window the room opens on)
 * also records the room's head span in the device store.
 */
export async function getChannelWindowEvents(
  channelId: string,
  cursor: ChannelPageCursor | null = null,
  limitRows = 50,
  options: { storeHead?: boolean } = {},
): Promise<RelayEvent[]> {
  const request = () =>
    invokeTauri<RelayEvent[]>("get_channel_window", {
      channelId,
      limitRows,
      cursor: cursor
        ? { created_at: cursor.createdAt, event_id: cursor.eventId }
        : null,
    });
  if (cursor || !recordsStoreHead(options, limitRows)) return request();
  return requestWithStoreHeads([channelId], request, (events) => {
    const answer = roomWindowAnswer(channelId, events);
    return answer ? [answer] : [];
  });
}

/**
 * Fetch the newest window of several channels in one relay round trip, keyed
 * by channel. Channels the relay skipped (no access) are absent. With
 * `storeHead` (the preload), records the head span of each window in the
 * device store.
 */
export async function getChannelWindowsEvents(
  channelIds: readonly string[],
  limitRows: number,
  options: { storeHead?: boolean } = {},
): Promise<Map<string, RelayEvent[]>> {
  const request = async () => {
    const entries = await invokeTauri<
      Array<{ channel_id: string; events: RelayEvent[] }>
    >("get_channel_windows", { channelIds, limitRows });
    return new Map(
      entries.map((entry) => [entry.channel_id, entry.events] as const),
    );
  };
  if (!recordsStoreHead(options, limitRows)) return request();
  const windows = await requestWithStoreHeads(channelIds, request, (windows) =>
    [...windows].flatMap(([channelId, events]) => {
      const answer = roomWindowAnswer(channelId, events);
      return answer ? [answer] : [];
    }),
  );
  return windows;
}
