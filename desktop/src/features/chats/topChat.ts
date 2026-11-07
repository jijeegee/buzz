import { readChatsViewMode } from "@/features/chats/chatListGroups";
import {
  type ChatListRow,
  orderChatRows,
} from "@/features/chats/chatListState";
import { isJoinedChat, sortChatList } from "@/features/sidebar/lib/chatList";
import type { Channel } from "@/shared/api/types";

export type TopChat = { channelId: string; threadRootId: string | null };

/**
 * The row at the top of the Chats list, as ChatList orders it: pinned rows,
 * then the relay's rows by activity, then rooms the relay did not send. In
 * the By channel view the top is a room, never one of its threads.
 */
export function topChatTarget({
  channels,
  mutedRootIds,
  pinnedThreadIds,
  rows,
  starredChannelIds,
}: {
  channels: readonly Channel[];
  mutedRootIds?: ReadonlySet<string>;
  pinnedThreadIds?: ReadonlySet<string>;
  rows: readonly ChatListRow[] | undefined;
  starredChannelIds?: ReadonlySet<string>;
}): TopChat | null {
  const listed = sortChatList(
    channels.filter(
      (channel) =>
        channel.channelType !== "forum" &&
        isJoinedChat(channel) &&
        !channel.archivedAt,
    ),
    starredChannelIds,
  );
  const listedIds = new Set(listed.map((channel) => channel.id));
  const room = (channelId: string): TopChat => ({
    channelId,
    threadRootId: null,
  });
  const first = (rows ?? [])
    .filter((row) => listedIds.has(row.channel_id))
    .filter(
      (row) =>
        row.type === "room" ||
        (row.root_id !== null && !mutedRootIds?.has(row.root_id)),
    );
  const ordered = orderChatRows(first, starredChannelIds, pinnedThreadIds);
  const top = ordered[0];
  const isPinned = (row: ChatListRow) =>
    row.type === "room"
      ? (starredChannelIds?.has(row.channel_id) ?? false)
      : row.root_id !== null && (pinnedThreadIds?.has(row.root_id) ?? false);
  const shownIds = new Set(ordered.map((row) => row.channel_id));
  const localPinned = listed.find(
    (channel) =>
      !shownIds.has(channel.id) && starredChannelIds?.has(channel.id),
  );
  // Pinned rooms the relay did not send sit after its pinned rows.
  if (localPinned && !(top && isPinned(top))) return room(localPinned.id);
  if (top) {
    const byChannel =
      readChatsViewMode(
        typeof window === "undefined" ? undefined : window.localStorage,
      ) === "by-channel";
    return top.type === "thread" && !byChannel
      ? { channelId: top.channel_id, threadRootId: top.root_id }
      : room(top.channel_id);
  }
  return listed[0] ? room(listed[0].id) : null;
}
