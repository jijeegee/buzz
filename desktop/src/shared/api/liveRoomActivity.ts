/**
 * How many live events each room, and each thread, has received in this
 * run: the window check compares it before and after a check to tell
 * whether live rows arrived while the check was out
 * (docs/message-sync.md#window-check).
 */
import { getThreadReference } from "@/features/messages/lib/threading";
import type { RelayEvent } from "@/shared/api/types";

const counts = new Map<string, number>();

function bump(key: string) {
  counts.set(key, (counts.get(key) ?? 0) + 1);
}

/** The counter key of `roomId`, or of its thread `rootId`. */
export function liveActivityKey(roomId: string, rootId: string | null) {
  return rootId ? `${roomId}/${rootId}` : roomId;
}

/** A live event of `roomId` arrived (socket REQ or user stream). */
export function noteLiveRoomActivity(roomId: string, event?: RelayEvent) {
  bump(roomId);
  const rootId = event?.tags ? getThreadReference(event.tags).rootId : null;
  if (rootId) bump(liveActivityKey(roomId, rootId));
}

/** A live event arrived; its room is its `h` tag, if any. */
export function noteLiveEventActivity(event: RelayEvent) {
  const roomId = event.tags?.find((tag) => tag[0] === "h")?.[1];
  if (roomId) noteLiveRoomActivity(roomId, event);
}

/** Live events received for `key` (a room, or a room's thread) so far. */
export function liveRoomActivity(key: string): number {
  return counts.get(key) ?? 0;
}
