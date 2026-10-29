/**
 * Opening a room from the device store: when the store's newest window is
 * certain (docs/message-sync.md#opening-a-room-from-the-store), the first
 * fetch of a room takes it instead of waiting for the relay. Later fetches
 * (refresh after subscribing, reconnects) still ask the relay.
 */
import type { QueryClient } from "@tanstack/react-query";

import { channelMessagesKey } from "@/features/messages/lib/messageQueryKeys";
import { getChannelWindowEvents } from "@/shared/api/channelWindow";
import { storeLiveAt } from "@/shared/api/relayStoreCoverage";
import { invokeTauri } from "@/shared/api/tauri";
import type { RelayEvent } from "@/shared/api/types";

type StoreRoomHeadDeps = {
  liveAt: (roomId: string) => number | null;
  roomHead: (roomId: string, liveAt: number) => Promise<RelayEvent[] | null>;
  relayWindow: (roomId: string) => Promise<RelayEvent[]>;
};

const defaultDeps: StoreRoomHeadDeps = {
  liveAt: storeLiveAt,
  roomHead: (roomId, liveAt) =>
    invokeTauri<RelayEvent[] | null>("message_store_room_head", {
      roomId,
      liveAt,
    }),
  relayWindow: (roomId) => getChannelWindowEvents(roomId),
};

let deps = defaultDeps;

/** Replace the store and relay bridge (tests). Returns the restore function. */
export function setStoreRoomHeadDeps(next: Partial<StoreRoomHeadDeps>) {
  const previous = deps;
  deps = { ...deps, ...next };
  return () => {
    deps = previous;
  };
}

/** The room's certain stored window, or `null` (then ask the relay). */
async function certainStoreWindow(channelId: string) {
  const liveAt = deps.liveAt(channelId);
  if (liveAt === null) return null;
  try {
    const events = await deps.roomHead(channelId, liveAt);
    return Array.isArray(events) && events.length > 0 ? events : null;
  } catch {
    return null;
  }
}

/** The newest window to open `channelId` with, in the relay's shape. */
export async function openChannelWindowEvents(
  queryClient: QueryClient,
  channelId: string,
): Promise<RelayEvent[]> {
  const fetchedBefore =
    (queryClient.getQueryState(channelMessagesKey(channelId))?.dataUpdatedAt ??
      0) > 0;
  if (!fetchedBefore) {
    const stored = await certainStoreWindow(channelId);
    if (stored) return stored;
  }
  return deps.relayWindow(channelId);
}
