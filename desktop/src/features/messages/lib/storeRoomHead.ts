/**
 * Opening a room from the device store: when the store's newest window is
 * certain (docs/message-sync.md#opening-a-room-from-the-store), the room's
 * first fetch in this run takes it instead of waiting for the relay, and the
 * relay is asked right after (once: the refresh after the room's live
 * subscription joins that request). Later fetches ask the relay.
 */
import { CancelledError, type QueryClient } from "@tanstack/react-query";

import { channelMessagesKey } from "@/features/messages/lib/messageQueryKeys";
import { logStoreComparison } from "@/features/messages/lib/storeCompare";
import {
  markStoredWindow,
  setStoreFollowUp,
  storeOpenLiveAt,
} from "@/features/messages/lib/storedWindows";
import { refreshChannelWindowMessages } from "@/features/messages/lib/projectChannelWindow";
import { sendRoomWindow } from "@/features/messages/lib/windowLiveToken";
import { getChannelWindowEvents } from "@/shared/api/channelWindow";
import { invokeTauri } from "@/shared/api/tauri";
import type { RelayEvent } from "@/shared/api/types";

type StoreRoomHeadDeps = {
  liveAt: (roomId: string) => number | null;
  roomHead: (roomId: string, liveAt: number) => Promise<RelayEvent[] | null>;
  relayWindow: (roomId: string) => Promise<RelayEvent[]>;
  relayRefresh: (queryClient: QueryClient, roomId: string) => Promise<void>;
};

const defaultDeps: StoreRoomHeadDeps = {
  liveAt: storeOpenLiveAt,
  roomHead: (roomId, liveAt) =>
    invokeTauri<RelayEvent[] | null>("message_store_room_head", {
      roomId,
      liveAt,
    }),
  relayWindow: (roomId) => getChannelWindowEvents(roomId),
  relayRefresh: (queryClient, roomId) =>
    refreshChannelWindowMessages(queryClient, roomId, false),
};

/**
 * Rooms fetched at least once in this run (from the store or the relay), per
 * query cache: a community switch starts a new one.
 */
let fetchedRooms = new WeakMap<QueryClient, Set<string>>();
/** Relay window requests started per room, to spot one overtaking a store read. */
let relayStarts = new WeakMap<QueryClient, Map<string, number>>();
/** A stored window awaiting the relay's, to compare them. */
let storedAwaitingRelay = new WeakMap<QueryClient, Map<string, RelayEvent[]>>();

function perClient<T>(
  map: WeakMap<QueryClient, T>,
  queryClient: QueryClient,
  make: () => T,
) {
  let value = map.get(queryClient);
  if (value === undefined) {
    value = make();
    map.set(queryClient, value);
  }
  return value;
}

function fetchedIn(queryClient: QueryClient) {
  return perClient(fetchedRooms, queryClient, () => new Set<string>());
}

/** A relay page for `channelId` landed some other way (preload). */
export function markStoreRoomHeadFetched(
  queryClient: QueryClient,
  channelId: string,
) {
  fetchedIn(queryClient).add(channelId);
}

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

/**
 * The store only draws the first frame: once this fetch has settled, the
 * relay's window replaces it as an ordinary refresh would.
 */
async function relayAfterSettle(queryClient: QueryClient, channelId: string) {
  await Promise.resolve();
  const query = queryClient
    .getQueryCache()
    .find({ queryKey: channelMessagesKey(channelId), exact: true });
  await query?.promise?.catch(() => {});
  try {
    await deps.relayRefresh(queryClient, channelId);
  } catch (error) {
    console.error("Failed to refresh a room opened from the store", error);
  }
  storedAwaitingRelay.get(queryClient)?.delete(channelId);
}

/** The relay's window, compared with a stored one it replaces. */
async function relayWindowFor(queryClient: QueryClient, channelId: string) {
  const starts = perClient(relayStarts, queryClient, () => new Map());
  starts.set(channelId, (starts.get(channelId) ?? 0) + 1);
  const waiting = perClient(storedAwaitingRelay, queryClient, () => new Map());
  const stored = waiting.get(channelId);
  waiting.delete(channelId);
  const events = await sendRoomWindow(queryClient, channelId, () =>
    deps.relayWindow(channelId),
  );
  if (stored) {
    logStoreComparison(`room ${channelId.slice(0, 8)}`, "room", stored, events);
  }
  return events;
}

/** Forget which rooms were fetched (tests). */
export function resetStoreRoomHeadRooms() {
  fetchedRooms = new WeakMap();
  relayStarts = new WeakMap();
  storedAwaitingRelay = new WeakMap();
}

/** The newest window to open `channelId` with, in the relay's shape. */
export async function openChannelWindowEvents(
  queryClient: QueryClient,
  channelId: string,
): Promise<RelayEvent[]> {
  const rooms = fetchedIn(queryClient);
  const first = !rooms.has(channelId);
  rooms.add(channelId);
  if (first) {
    const startsBefore = relayStarts.get(queryClient)?.get(channelId) ?? 0;
    const stored = await certainStoreWindow(channelId);
    // A refresh that cancelled this fetch has already asked the relay; this
    // fetch's answer is discarded, so it asks nothing more.
    if ((relayStarts.get(queryClient)?.get(channelId) ?? 0) !== startsBefore) {
      throw new CancelledError();
    }
    if (stored) {
      markStoredWindow(stored);
      perClient(storedAwaitingRelay, queryClient, () => new Map()).set(
        channelId,
        stored,
      );
      setStoreFollowUp(
        queryClient,
        channelId,
        relayAfterSettle(queryClient, channelId),
      );
      return stored;
    }
  }
  return relayWindowFor(queryClient, channelId);
}
