import { useMemo, type ComponentProps } from "react";
import {
  chatRowContextId,
  chatRowUnread,
  orderChatRows,
  type ChatListRow,
} from "@/features/chats/chatListState";
import { isJoinedChat, sortChatList } from "@/features/sidebar/lib/chatList";
import type { Channel } from "@/shared/api/types";
import { ChatThreadRow } from "./ChatThreadRow";
import { SidebarSection, type SidebarSectionRow } from "./SidebarSection";

/** The relay's chats list, present only while its chat-state switch is on. */
export type ServerChats = {
  rows: readonly ChatListRow[];
  /** This device's own read position for a read-state context. */
  getReadAt: (contextId: string) => number | null;
  onOpenThread: (channelId: string, rootId: string) => void;
  /** Changes whenever local read positions change. */
  readStateVersion: number;
  pinnedThreadIds?: ReadonlySet<string>;
  /** Rooms marked unread on this device: shown unread before the relay agrees. */
  forcedUnreadChannelIds?: ReadonlySet<string>;
  /** Threads muted (unfollowed) on this device stay out of the list. */
  mutedRootIds?: ReadonlySet<string>;
};

type ChatListProps = Omit<
  ComponentProps<typeof SidebarSection>,
  "title" | "testId" | "rows"
> & { serverChats?: ServerChats };

function isListedChat(channel: Channel) {
  return (
    channel.channelType !== "forum" &&
    isJoinedChat(channel) &&
    !channel.archivedAt
  );
}

/**
 * Server rows in chats-list order with counts from the relay. Rooms the
 * relay did not send (beyond the loaded pages) follow in today's order, so
 * no room ever drops out of the list.
 */
function buildServerChatRows(
  server: ServerChats,
  localItems: readonly Channel[],
  props: ChatListProps,
) {
  const channelById = new Map(
    localItems.map((channel) => [channel.id, channel]),
  );
  const unreadIds = new Set<string>();
  const counts = new Map<string, number>();
  const pinned: SidebarSectionRow[] = [];
  const rest: SidebarSectionRow[] = [];
  const shown = new Set<string>();
  for (const row of orderChatRows(
    server.rows,
    props.starredChannelIds,
    server.pinnedThreadIds,
  )) {
    const channel = channelById.get(row.channel_id);
    if (!channel) continue;
    const unread = chatRowUnread(row, server.getReadAt(chatRowContextId(row)));
    const isMuted = props.mutedChannelIds?.has(channel.id) ?? false;
    if (row.type === "room") {
      if (shown.has(channel.id)) continue;
      shown.add(channel.id);
      const forced = server.forcedUnreadChannelIds?.has(channel.id) ?? false;
      if (
        forced ||
        ((unread.count > 0 || unread.marked) &&
          (!isMuted || unread.mentions > 0))
      ) {
        unreadIds.add(channel.id);
        counts.set(channel.id, unread.count);
      }
      const target = props.starredChannelIds?.has(channel.id) ? pinned : rest;
      target.push({ kind: "channel", channel });
      continue;
    }
    if (!row.root_id || server.mutedRootIds?.has(row.root_id)) continue;
    const isPinned = server.pinnedThreadIds?.has(row.root_id) ?? false;
    (isPinned ? pinned : rest).push({
      kind: "custom",
      key: `thread:${row.root_id}`,
      node: (
        <ChatThreadRow
          channelLabel={props.channelLabels?.[channel.id] ?? channel.name}
          isDm={channel.channelType === "dm"}
          isMuted={isMuted}
          isPinned={isPinned}
          onOpenThread={server.onOpenThread}
          row={row}
          unread={unread}
        />
      ),
    });
  }
  for (const channel of localItems) {
    if (shown.has(channel.id)) continue;
    const target = props.starredChannelIds?.has(channel.id) ? pinned : rest;
    target.push({ kind: "channel", channel });
  }
  return { counts, rows: [...pinned, ...rest], unreadIds };
}

/**
 * The selected community's joined channels and visible DMs in one chat list.
 * With `serverChats` (the relay keeps chat state) rows, followed threads and
 * unread counts come from the relay; without it, this is today's list.
 */
export function ChatList({ serverChats, ...props }: ChatListProps) {
  const items = useMemo(
    () =>
      sortChatList(props.items.filter(isListedChat), props.starredChannelIds),
    [props.items, props.starredChannelIds],
  );
  // biome-ignore lint/correctness/useExhaustiveDependencies: readStateVersion signals that local read positions moved
  const server = useMemo(
    () => (serverChats ? buildServerChatRows(serverChats, items, props) : null),
    [
      serverChats,
      serverChats?.readStateVersion,
      items,
      props.channelLabels,
      props.mutedChannelIds,
      props.starredChannelIds,
    ],
  );
  if (!server) {
    return (
      <SidebarSection
        {...props}
        items={items}
        title="Chats"
        testId="chat-list"
      />
    );
  }
  return (
    <SidebarSection
      {...props}
      items={items}
      rows={server.rows}
      title="Chats"
      testId="chat-list"
      unreadChannelCounts={server.counts}
      unreadChannelIds={server.unreadIds}
    />
  );
}
