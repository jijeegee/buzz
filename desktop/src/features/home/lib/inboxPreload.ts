import type { InboxItem } from "@/features/home/lib/inbox";
import { getInboxRoomEntry } from "@/features/home/lib/inboxRoomEntry";

/** Inbox rows whose rooms load in the background: about one screenful. */
export const INBOX_PRELOAD_ROWS = 12;
/** Newest messages loaded per preloaded room, about one chat screen. */
export const INBOX_PRELOAD_MESSAGES = 20;

export type InboxPreloadTarget = {
  channelId: string;
  /** Thread rows only: the thread root and the reply the row opens on. */
  thread: { rootId: string; replyId: string } | null;
};

/** The rooms (and threads) behind the top chat-list rows, in display order. */
export function inboxPreloadTargets(
  items: readonly InboxItem[],
  limit = INBOX_PRELOAD_ROWS,
): InboxPreloadTarget[] {
  const targets: InboxPreloadTarget[] = [];
  for (const item of items.slice(0, limit)) {
    const entry = getInboxRoomEntry(item, "conversations");
    if (!entry) continue;
    // In the chat list only thread rows open on a specific message.
    targets.push({
      channelId: entry.channelId,
      thread: entry.messageId
        ? { rootId: item.conversationId, replyId: entry.messageId }
        : null,
    });
  }
  return targets;
}

export function inboxPreloadTargetKey(target: InboxPreloadTarget): string {
  return target.thread
    ? `${target.channelId}:${target.thread.rootId}`
    : target.channelId;
}
