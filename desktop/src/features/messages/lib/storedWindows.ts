/**
 * Opening rooms and threads from the device store: the build switch, and
 * which window answers came from the store rather than the relay. Stored
 * answers are drawn, but only a relay answer is persisted as a head cache.
 */
import type { QueryClient } from "@tanstack/react-query";

import { storeLiveAt } from "@/shared/api/relayStoreCoverage";
import type { RelayEvent } from "@/shared/api/types";

const stored = new WeakSet<readonly RelayEvent[]>();

export function markStoredWindow(events: readonly RelayEvent[]) {
  stored.add(events);
}

export function isStoredWindow(events: readonly RelayEvent[]) {
  return stored.has(events);
}

/**
 * Stored windows drawn before the relay confirmed them (a paint,
 * docs/message-sync.md#painting-from-the-store): they never count as a
 * fetched window, so where the room opens stays provisional until the
 * relay's answer is shown.
 */
const painted = new WeakSet<readonly RelayEvent[]>();

export function markPaintedWindow(events: readonly RelayEvent[]) {
  stored.add(events);
  painted.add(events);
}

export function isPaintedWindow(events: readonly RelayEvent[]) {
  return painted.has(events);
}

/** A build with `VITE_KUZZ_STORE_OPEN=off` opens everything from the relay. */
export function storeOpensEnabled(): boolean {
  return import.meta.env?.VITE_KUZZ_STORE_OPEN !== "off";
}

/** When `roomId`'s live REQ reached EOSE, or null when store opens are off. */
export function storeOpenLiveAt(roomId: string): number | null {
  return storeOpensEnabled() ? storeLiveAt(roomId) : null;
}

/**
 * The relay request that follows a room's store open, while it runs. Any
 * other refresh of the room in that time (the one after its live
 * subscription is set up) joins it instead of asking again.
 */
const followUps = new WeakMap<QueryClient, Map<string, Promise<void>>>();

export function setStoreFollowUp(
  queryClient: QueryClient,
  channelId: string,
  followUp: Promise<void>,
) {
  let rooms = followUps.get(queryClient);
  if (!rooms) {
    rooms = new Map();
    followUps.set(queryClient, rooms);
  }
  const map = rooms;
  map.set(channelId, followUp);
  void followUp.finally(() => {
    if (map.get(channelId) === followUp) map.delete(channelId);
  });
}

export function storeFollowUp(
  queryClient: QueryClient,
  channelId: string,
): Promise<void> | undefined {
  return followUps.get(queryClient)?.get(channelId);
}
