import type { InboxFilter, InboxItem } from "@/features/home/lib/inbox";
import { isChannelMainRoomItem } from "@/features/home/lib/inboxViewHelpers";
import { isProjectInboxItem } from "@/features/home/lib/projectInbox";
import type { FeedItem } from "@/shared/api/types";
import {
  getThreadReference,
  isBroadcastReply,
} from "@/features/messages/lib/threading";
import {
  KIND_STREAM_MESSAGE,
  KIND_STREAM_MESSAGE_V2,
} from "@/shared/constants/kinds";

/** Filters whose rows are chat messages and so enter their chat room. */
const ROOM_FILTERS: ReadonlySet<InboxFilter> = new Set([
  "all",
  "mention",
  "thread",
  "conversations",
]);

const CHAT_MESSAGE_KINDS: ReadonlySet<number> = new Set([
  KIND_STREAM_MESSAGE,
  KIND_STREAM_MESSAGE_V2,
]);

/** Where an inbox row enters the chat screen. */
export type InboxRoomEntry = {
  channelId: string;
  /** Message to land on; a thread reply opens its thread. Null opens the room. */
  messageId: string | null;
};

/**
 * The inbox is another way into the chat screen Chats uses, so a chat message
 * row enters that room rather than a separate inbox view. A thread row lands in
 * its thread; a room row (a channel main timeline or a DM) just opens the room;
 * a single message elsewhere lands on it, as a message link does.
 *
 * Work that is not a chat message keeps the inbox's own detail and returns
 * null: approval requests, agent updates, project items, forum posts, and the
 * Needs action / Agents / Projects / Drafts / Reminders filters.
 */
export function getInboxRoomEntry(
  item: InboxItem,
  filter: InboxFilter,
): InboxRoomEntry | null {
  const channelId = item.item.channelId;
  if (
    !channelId ||
    !ROOM_FILTERS.has(filter) ||
    item.isActionRequired ||
    !item.groupItems.every((event) => CHAT_MESSAGE_KINDS.has(event.kind)) ||
    item.groupItems.some(isProjectInboxItem)
  ) {
    return null;
  }

  const isThreadReply =
    getThreadReference(item.item.tags).parentId !== null &&
    !isBroadcastReply(item.item.tags);
  if (isThreadReply) return { channelId, messageId: item.item.id };

  const isRoomRow =
    item.item.channelType === "dm" ||
    isChannelMainRoomItem(item) ||
    filter === "conversations";
  return { channelId, messageId: isRoomRow ? null : item.item.id };
}

/**
 * Feed-event form of the same rule, for the selection anchor before its row is
 * resolved: true when the event will open in its chat room, so the inbox need
 * not load its own context for it.
 */
export function opensInChatRoom(event: FeedItem, filter: InboxFilter) {
  return Boolean(
    event.channelId &&
      ROOM_FILTERS.has(filter) &&
      CHAT_MESSAGE_KINDS.has(event.kind) &&
      event.category !== "needs_action" &&
      !isProjectInboxItem(event),
  );
}
