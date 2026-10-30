/**
 * Opening a room from the device store, paint first
 * (docs/message-sync.md#painting-from-the-store): the room's first fetch in
 * this run reads its stored window and asks the relay at the same time.
 * Stored rows that answer first are drawn at once, not yet confirmed, and the
 * relay's answer replaces them as the refresh that follows (the refresh after
 * the room's live subscription joins it). Later fetches ask the relay.
 */
import { CancelledError, type QueryClient } from "@tanstack/react-query";

import { channelMessagesKey } from "@/features/messages/lib/messageQueryKeys";
import {
  forgetPaintCoverage,
  notePaintCoverage,
  storeScope,
} from "@/features/messages/lib/paintCoverage";
import { storeBeforeRelay } from "@/features/messages/lib/paintFirst";
import { logStoreComparison } from "@/features/messages/lib/storeCompare";
import {
  markPaintedWindow,
  setStoreFollowUp,
  storeOpensEnabled,
} from "@/features/messages/lib/storedWindows";
import { refreshChannelWindowMessages } from "@/features/messages/lib/projectChannelWindow";
import { sendRoomWindow } from "@/features/messages/lib/windowLiveToken";
import { getChannelWindowEvents } from "@/shared/api/channelWindow";
import { invokeTauri } from "@/shared/api/tauri";
import type { RelayEvent } from "@/shared/api/types";

/** A room's stored window and from when its room is held without a gap. */
export type RoomPaint = { window: RelayEvent[]; completeFrom: number | null };

type StoreRoomHeadDeps = {
  /**
   * The room's newest stored window, certain or not, read in the account
   * and server of `queryClient`; null when none.
   */
  roomPaint: (
    queryClient: QueryClient,
    roomId: string,
  ) => Promise<RoomPaint | null>;
  relayWindow: (roomId: string) => Promise<RelayEvent[]>;
  relayRefresh: (queryClient: QueryClient, roomId: string) => Promise<void>;
};

const defaultDeps: StoreRoomHeadDeps = {
  roomPaint: async (queryClient, roomId) => {
    const scope = storeScope(queryClient);
    if (!storeOpensEnabled() || !scope) return null;
    const paint = await invokeTauri<{
      window: RelayEvent[];
      complete_from: number | null;
    } | null>("message_store_room_paint", {
      roomId,
      account: scope.pubkey,
      relayUrl: scope.relayUrl,
    });
    return paint
      ? { window: paint.window, completeFrom: paint.complete_from ?? null }
      : null;
  },
  relayWindow: (roomId) =>
    getChannelWindowEvents(roomId, null, 50, { storeHead: true }),
  relayRefresh: (queryClient, roomId) =>
    refreshChannelWindowMessages(queryClient, roomId, false),
};

/**
 * Rooms fetched at least once in this run (from the store or the relay), per
 * query cache: a community switch starts a new one.
 */
let fetchedRooms = new WeakMap<QueryClient, Set<string>>();
/**
 * Relay window requests started or taken over per room, to spot a refresh
 * overtaking a first fetch.
 */
let relayStarts = new WeakMap<QueryClient, Map<string, number>>();
/** A first fetch's relay request, until a later fetch takes it. */
let pendingRelays = new WeakMap<
  QueryClient,
  Map<string, Promise<RelayEvent[]>>
>();
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

function pendingIn(queryClient: QueryClient) {
  return perClient(pendingRelays, queryClient, () => new Map());
}

function noteStart(queryClient: QueryClient, channelId: string) {
  const starts = perClient(relayStarts, queryClient, () => new Map());
  const count = (starts.get(channelId) ?? 0) + 1;
  starts.set(channelId, count);
  return count;
}

function startsOf(queryClient: QueryClient, channelId: string) {
  return relayStarts.get(queryClient)?.get(channelId) ?? 0;
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

/** The room's stored window to paint, or `null` when nothing is stored. */
async function paintWindow(queryClient: QueryClient, channelId: string) {
  const paint = await deps.roomPaint(queryClient, channelId);
  return Array.isArray(paint?.window) && paint.window.length > 0 ? paint : null;
}

/** Sends the room's relay window request. */
function startRelay(queryClient: QueryClient, channelId: string) {
  noteStart(queryClient, channelId);
  return sendRoomWindow(queryClient, channelId, () =>
    deps.relayWindow(channelId),
  );
}

/**
 * The stored rows only draw the first frame: once this fetch has settled,
 * the relay's window replaces them as an ordinary refresh would. That
 * refresh takes the request already out (`relay`).
 */
async function relayAfterSettle(
  queryClient: QueryClient,
  channelId: string,
  relay: Promise<RelayEvent[]>,
) {
  await Promise.resolve();
  const query = queryClient
    .getQueryCache()
    .find({ queryKey: channelMessagesKey(channelId), exact: true });
  await query?.promise?.catch(() => {});
  const pending = pendingRelays.get(queryClient);
  // A refetch in the meantime already took the request: refreshing again
  // would ask the relay a second time.
  if (pending?.get(channelId) === relay) {
    try {
      await deps.relayRefresh(queryClient, channelId);
    } catch (error) {
      console.error("Failed to refresh a room opened from the store", error);
    }
  }
  // A room closed meanwhile was not refetched: its request is not kept.
  if (pending?.get(channelId) === relay) pending.delete(channelId);
  storedAwaitingRelay.get(queryClient)?.delete(channelId);
  forgetPaintCoverage(channelId);
}

/**
 * The relay's window: the first fetch's request when it is still out, else
 * a new one. Compared with a stored window it replaces.
 */
async function relayWindowFor(queryClient: QueryClient, channelId: string) {
  const pending = pendingIn(queryClient);
  const out = pending.get(channelId);
  pending.delete(channelId);
  let events: RelayEvent[];
  try {
    if (out) {
      noteStart(queryClient, channelId);
      events = await out;
    } else {
      events = await startRelay(queryClient, channelId);
    }
  } finally {
    forgetPaintCoverage(channelId);
  }
  const waiting = storedAwaitingRelay.get(queryClient);
  const stored = waiting?.get(channelId);
  if (stored) {
    waiting?.delete(channelId);
    logStoreComparison(`room ${channelId.slice(0, 8)}`, "room", stored, events);
  }
  return events;
}

/** Forget which rooms were fetched (tests). */
export function resetStoreRoomHeadRooms() {
  fetchedRooms = new WeakMap();
  relayStarts = new WeakMap();
  pendingRelays = new WeakMap();
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
  if (!first) return relayWindowFor(queryClient, channelId);

  // Sent once the effects of the commit that opened the room have run, so a
  // room listener attached in that commit goes out with the request
  // (windowLiveToken.ts) and no refresh is needed after it.
  await Promise.resolve();
  const relay = startRelay(queryClient, channelId);
  const pending = pendingIn(queryClient);
  pending.set(channelId, relay);
  const startsBefore = startsOf(queryClient, channelId);
  const race = await storeBeforeRelay(
    paintWindow(queryClient, channelId),
    relay,
  );
  // A refresh that took the relay request over has cancelled this fetch;
  // its answer is discarded, so it draws and asks nothing.
  if (startsOf(queryClient, channelId) !== startsBefore) {
    throw new CancelledError();
  }
  if (!race || race.relayFailed) {
    if (pending.get(channelId) === relay) pending.delete(channelId);
    if (!race) return relay;
  }
  const stored = race.stored.window;
  markPaintedWindow(stored);
  if (race.relayFailed) return stored;
  notePaintCoverage(channelId, race.stored.completeFrom);
  perClient(storedAwaitingRelay, queryClient, () => new Map()).set(
    channelId,
    stored,
  );
  setStoreFollowUp(
    queryClient,
    channelId,
    relayAfterSettle(queryClient, channelId, relay),
  );
  return stored;
}
