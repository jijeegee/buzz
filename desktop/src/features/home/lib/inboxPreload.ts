import type { InboxItem } from "@/features/home/lib/inbox";
import { getInboxRoomEntry } from "@/features/home/lib/inboxRoomEntry";
import { getThreadReference } from "@/features/messages/lib/threading";

/** Inbox rows whose rooms load first: about one screenful. */
export const INBOX_PRELOAD_ROWS = 12;
/** Rows whose rooms load in the background after the first wave. */
export const INBOX_PRELOAD_BACKGROUND_ROWS = 48;
/** Newest messages loaded per preloaded room: a whole open window. */
export const INBOX_PRELOAD_MESSAGES = 50;

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
    const replyId = entry.messageId;
    // DM rows group under `dm:<channel>`, so take the root from the reply.
    const reply = replyId
      ? item.groupItems.find((event) => event.id === replyId)
      : undefined;
    const rootId =
      (reply && getThreadReference(reply.tags).rootId) ?? item.conversationId;
    targets.push({
      channelId: entry.channelId,
      thread: replyId ? { rootId, replyId } : null,
    });
  }
  return targets;
}

export function inboxPreloadTargetKey(target: InboxPreloadTarget): string {
  return target.thread
    ? `${target.channelId}:${target.thread.rootId}`
    : target.channelId;
}
