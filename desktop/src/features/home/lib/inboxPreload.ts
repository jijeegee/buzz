import type { InboxItem } from "@/features/home/lib/inbox";
import { getInboxRoomEntry } from "@/features/home/lib/inboxRoomEntry";

/** Inbox rows whose rooms load in the background: about one screenful. */
export const INBOX_PRELOAD_ROWS = 12;
/** Newest messages loaded per preloaded room, about one chat screen. */
export const INBOX_PRELOAD_MESSAGES = 20;

export type InboxPreloadTarget = {
  channelId: string;
  /** Thread root for thread rows; null for a room's main timeline or a DM. */
  rootId: string | null;
};

const ROOM_ROW_PREFIXES = ["channel:", "dm:"];

/** The rooms (and threads) behind the top chat-list rows, in display order. */
export function inboxPreloadTargets(
  items: readonly InboxItem[],
  limit = INBOX_PRELOAD_ROWS,
): InboxPreloadTarget[] {
  const targets: InboxPreloadTarget[] = [];
  for (const item of items.slice(0, limit)) {
    const entry = getInboxRoomEntry(item, "conversations");
    if (!entry) continue;
    const isRoomRow = ROOM_ROW_PREFIXES.some((prefix) =>
      item.conversationId.startsWith(prefix),
    );
    targets.push({
      channelId: entry.channelId,
      rootId: isRoomRow ? null : item.conversationId,
    });
  }
  return targets;
}

export function inboxPreloadTargetKey(target: InboxPreloadTarget): string {
  return target.rootId
    ? `${target.channelId}:${target.rootId}`
    : target.channelId;
}
