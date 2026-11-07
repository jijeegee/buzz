/**
 * How many live events each room has received in this run: the window check
 * compares it before and after a check to tell whether live rows arrived
 * while the check was out (docs/message-sync.md#window-check).
 */
import type { RelayEvent } from "@/shared/api/types";

const counts = new Map<string, number>();

/** A live event of `roomId` arrived (socket REQ or user stream). */
export function noteLiveRoomActivity(roomId: string) {
  counts.set(roomId, (counts.get(roomId) ?? 0) + 1);
}

/** A live event arrived; its room is its `h` tag, if any. */
export function noteLiveEventActivity(event: RelayEvent) {
  const roomId = event.tags?.find((tag) => tag[0] === "h")?.[1];
  if (roomId) noteLiveRoomActivity(roomId);
}

/** Live events received for `roomId` so far. */
export function liveRoomActivity(roomId: string): number {
  return counts.get(roomId) ?? 0;
}
