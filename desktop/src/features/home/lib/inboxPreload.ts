import type { InboxItem } from "@/features/home/lib/inbox";
import { getInboxRoomEntry } from "@/features/home/lib/inboxRoomEntry";
import { getThreadReference } from "@/features/messages/lib/threading";

/** Inbox rows whose rooms load in the background: about one screenful. */
export const INBOX_PRELOAD_ROWS = 12;
/** Newest messages loaded per preloaded room, about one chat screen. */
export const INBOX_PRELOAD_MESSAGES = 20;
/** Most recent rooms with unread whose history loads down to the marker. */
export const INBOX_UNREAD_ANCHOR_ROOMS = 20;

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

/**
 * The most recent rooms with unread messages (room rows, not threads) and
 * their read markers, newest activity first. Rooms with no marker (never
 * read) open at the newest message and are left out.
 */
export function inboxUnreadAnchorTargets(
  items: readonly InboxItem[],
  readAtFor: (channelId: string) => number | null,
  limit = INBOX_UNREAD_ANCHOR_ROOMS,
): { channelId: string; readAt: number }[] {
  const targets: { channelId: string; readAt: number }[] = [];
  const seen = new Set<string>();
  for (const item of items) {
    if (targets.length >= limit) break;
    if (item.unreadCount <= 0) continue;
    const entry = getInboxRoomEntry(item, "conversations");
    if (!entry || entry.messageId || seen.has(entry.channelId)) continue;
    seen.add(entry.channelId);
    const readAt = readAtFor(entry.channelId);
    if (readAt !== null) targets.push({ channelId: entry.channelId, readAt });
  }
  return targets;
}
