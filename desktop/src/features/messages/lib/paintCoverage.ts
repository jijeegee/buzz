/**
 * What a store paint needs besides its rows
 * (docs/message-sync.md#painting-from-the-store): the account and server a
 * query cache belongs to, so a paint never reads another account's store
 * during a switch, and how far back a painted window's room is held without
 * a gap, so a window with a hole in it never decides where the open lands.
 */
import type { QueryClient } from "@tanstack/react-query";

export type StoreScope = { pubkey: string; relayUrl: string };

const scopes = new WeakMap<QueryClient, StoreScope>();

/** The account and server `queryClient` caches (one per community). */
export function setStoreScope(queryClient: QueryClient, scope: StoreScope) {
  scopes.set(queryClient, scope);
}

/** `queryClient`'s account and server; a cache without one paints nothing. */
export function storeScope(queryClient: QueryClient): StoreScope | null {
  return scopes.get(queryClient) ?? null;
}

/**
 * From when each painted room or thread is held without a gap, while its
 * paint awaits the relay: `null` when not known.
 */
const coverage = new Map<string, number | null>();

export function threadPaintKey(channelId: string, rootId: string) {
  return `${channelId}/${rootId}`;
}

export function notePaintCoverage(key: string, completeFrom: number | null) {
  coverage.set(key, completeFrom);
}

/** The relay answered (or failed): the rows held decide as they are. */
export function forgetPaintCoverage(key: string) {
  coverage.delete(key);
}

/**
 * For a room (channel id) or thread (threadPaintKey) shown from a paint the
 * relay has not replaced yet: from when it is held without a gap (`null`:
 * unknown). `undefined` when no such paint is shown.
 */
export function paintCoverage(key: string | null): number | null | undefined {
  return key === null ? undefined : coverage.get(key);
}

/** Forget every paint (tests). */
export function resetPaintCoverage() {
  coverage.clear();
}
