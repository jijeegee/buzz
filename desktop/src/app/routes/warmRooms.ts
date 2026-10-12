export type WarmRooms = {
  /** Most recently open first; decides which room to drop. */
  recency: string[];
  /** Render order. Never reordered: moving a hidden room's DOM resets scroll. */
  order: string[];
};

/** The open room plus the most recently left ones, `limit` rooms in all. */
export function nextWarmRooms(
  current: WarmRooms,
  channelId: string,
  limit: number,
): WarmRooms {
  const recency = [
    channelId,
    ...current.recency.filter((id) => id !== channelId),
  ].slice(0, Math.max(1, limit));
  const kept = current.order.filter((id) => recency.includes(id));
  return {
    recency,
    order: kept.includes(channelId) ? kept : [...kept, channelId],
  };
}
