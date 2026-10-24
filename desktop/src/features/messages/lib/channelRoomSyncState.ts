import type { QueryClient } from "@tanstack/react-query";

/**
 * Rooms whose window store was cut back to its newest live rows (no pages):
 * their in-memory history no longer reaches the relay page they held, so the
 * next authoritative newest-window fetch must replace it. Cleared by that
 * fetch (`reconcileFetchedChannelWindow`).
 */
const needsRefresh = new WeakMap<QueryClient, Set<string>>();

export function markChannelNeedsRefresh(
  queryClient: QueryClient,
  channelId: string,
): void {
  const channels = needsRefresh.get(queryClient);
  if (channels) channels.add(channelId);
  else needsRefresh.set(queryClient, new Set([channelId]));
}

export function channelNeedsRefresh(
  queryClient: QueryClient,
  channelId: string,
): boolean {
  return needsRefresh.get(queryClient)?.has(channelId) ?? false;
}

export function clearChannelNeedsRefresh(
  queryClient: QueryClient,
  channelId: string,
): void {
  needsRefresh.get(queryClient)?.delete(channelId);
}
