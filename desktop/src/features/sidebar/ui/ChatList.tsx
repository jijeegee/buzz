import { useCallback, useMemo, useState, type ComponentProps } from "react";
import {
  type ChatWorking,
  splitChatWorking,
} from "@/features/chats/chatWorking";
import {
  chatRowPreviewLine,
  type ChatRowPreview,
} from "@/features/chats/chatRowPreview";
import {
  chatRowContextId,
  chatRowUnread,
  orderChatRows,
  type ChatListRow,
} from "@/features/chats/chatListState";
import { useBlockedPeople } from "@/features/people/BlockedPeopleContext";
import { isJoinedChat, sortChatList } from "@/features/sidebar/lib/chatList";
import type { Channel } from "@/shared/api/types";
import {
  groupChatRowsByChannel,
  type ChatsViewMode,
} from "@/features/chats/chatListGroups";
import { ChatMoreThreadsRow } from "./ChatMoreThreadsRow";
import { ChatThreadRow } from "./ChatThreadRow";
import type { ChatThreadRowActions } from "./ChatThreadRowMenu";
import { SidebarSection, type SidebarSectionRow } from "./SidebarSection";

/** The relay's chats list, present only while its chat-state switch is on. */
export type ServerChats = {
  rows: readonly ChatListRow[];
  /** This device's own read position for a read-state context. */
  getReadAt: (contextId: string) => number | null;
  onOpenThread: (channelId: string, rootId: string) => void;
  /** The thread open in the main pane, if any; only its row is selected. */
  openRootId?: string | null;
  /** Changes whenever local read positions change. */
  readStateVersion: number;
  pinnedThreadIds?: ReadonlySet<string>;
  /** Rooms marked unread on this device: shown unread before the relay agrees. */
  forcedUnreadChannelIds?: ReadonlySet<string>;
  /** Threads muted (unfollowed) on this device stay out of the list. */
  mutedRootIds?: ReadonlySet<string>;
  /** A thread row's menu; without it thread rows have no menu. */
  threadActions?: ChatThreadRowActions;
};

type ChatListProps = Omit<
  ComponentProps<typeof SidebarSection>,
  "title" | "testId" | "rows"
> & {
  serverChats?: ServerChats;
  /** All chats (default) or By channel; used only with `serverChats`. */
  view?: ChatsViewMode;
};

function isListedChat(channel: Channel) {
  return (
    channel.channelType !== "forum" &&
    isJoinedChat(channel) &&
    !channel.archivedAt
  );
}

type ByChannel = {
  expandedSteps: ReadonlyMap<string, number>;
  onShowLess: (channelId: string) => void;
  onShowMore: (channelId: string) => void;
};

/**
 * Server rows in chats-list order with counts from the relay. Rooms the
 * relay did not send (beyond the loaded pages) follow in today's order, so
 * no room ever drops out of the list. With `byChannel`, rows are grouped
 * under their channel (View B).
 */
function buildServerChatRows(
  server: ServerChats,
  localItems: readonly Channel[],
  props: ChatListProps,
  byChannel: ByChannel | null,
  workingByRootId: ChatWorking["threads"],
  isBlocked: (pubkey: string) => boolean,
) {
  const previewOf = (row: ChatListRow): ChatRowPreview => {
    const event = row.latest;
    if (!event) return { line: null, at: row.at > 0 ? row.at : null };
    return { at: row.at, line: chatRowPreviewLine(event, isBlocked) };
  };
  const channelById = new Map(
    localItems.map((channel) => [channel.id, channel]),
  );
  const unreadIds = new Set<string>();
  const counts = new Map<string, number>();
  const shown = new Set<string>();
  const listed: ChatListRow[] = [];
  const unreadOf = (row: ChatListRow) =>
    chatRowUnread(row, server.getReadAt(chatRowContextId(row)));
  for (const row of orderChatRows(
    server.rows,
    props.starredChannelIds,
    server.pinnedThreadIds,
  )) {
    const channel = channelById.get(row.channel_id);
    if (!channel) continue;
    if (row.type === "room") {
      if (shown.has(channel.id)) continue;
      shown.add(channel.id);
      const unread = unreadOf(row);
      const isMuted = props.mutedChannelIds?.has(channel.id) ?? false;
      const forced = server.forcedUnreadChannelIds?.has(channel.id) ?? false;
      if (
        forced ||
        ((unread.count > 0 || unread.marked) &&
          (!isMuted || unread.mentions > 0))
      ) {
        unreadIds.add(channel.id);
        counts.set(channel.id, unread.count);
      }
      listed.push(row);
      continue;
    }
    if (!row.root_id || server.mutedRootIds?.has(row.root_id)) continue;
    listed.push(row);
  }
  const isPinnedRoom = (channelId: string) =>
    props.starredChannelIds?.has(channelId) ?? false;
  const isPinnedThread = (rootId: string) =>
    server.pinnedThreadIds?.has(rootId) ?? false;
  const toSectionRow = (row: ChatListRow): SidebarSectionRow => {
    // Every listed row's channel is in channelById.
    const channel = channelById.get(row.channel_id) as Channel;
    if (row.type === "room") {
      return { kind: "channel", channel, preview: previewOf(row) };
    }
    const rootId = row.root_id ?? row.key;
    return {
      kind: "custom",
      key: `thread:${rootId}`,
      node: (
        <ChatThreadRow
          actions={server.threadActions}
          channelLabel={props.channelLabels?.[channel.id] ?? channel.name}
          indent={byChannel !== null}
          isActive={
            props.isActiveChannel === true &&
            props.selectedChannelId === channel.id &&
            server.openRootId === rootId
          }
          isDm={channel.channelType === "dm"}
          isMuted={props.mutedChannelIds?.has(channel.id) ?? false}
          isPinned={isPinnedThread(rootId)}
          onOpenThread={server.onOpenThread}
          preview={previewOf(row)}
          row={row}
          unread={unreadOf(row)}
          working={workingByRootId.get(rootId)}
        />
      ),
    };
  };
  const grouping = byChannel
    ? groupChatRowsByChannel(listed, {
        expandedSteps: byChannel.expandedSteps,
        isPinnedRoom,
        isPinnedThread,
        unreadOf,
      })
    : null;
  // A group whose main row is not on a loaded page starts with the local row.
  for (const entry of grouping?.entries ?? []) {
    if (entry.kind === "channel") shown.add(entry.channelId);
  }
  const localPinned: SidebarSectionRow[] = [];
  const localRest: SidebarSectionRow[] = [];
  for (const channel of localItems) {
    if (shown.has(channel.id)) continue;
    (isPinnedRoom(channel.id) ? localPinned : localRest).push({
      kind: "channel",
      channel,
      preview: localPreview(channel),
    });
  }
  let rows: SidebarSectionRow[];
  if (byChannel && grouping) {
    const { entries, pinnedEntries } = grouping;
    const grouped = entries.map((entry): SidebarSectionRow => {
      if (entry.kind === "row") return toSectionRow(entry.row);
      if (entry.kind === "channel") {
        const channel = channelById.get(entry.channelId) as Channel;
        return { kind: "channel", channel, preview: localPreview(channel) };
      }
      return {
        kind: "custom",
        key: `more:${entry.channelId}`,
        node: (
          <ChatMoreThreadsRow
            channelId={entry.channelId}
            count={entry.count}
            expanded={entry.expanded}
            isMuted={props.mutedChannelIds?.has(entry.channelId) ?? false}
            marked={entry.marked}
            mentions={entry.mentions}
            onShowLess={byChannel.onShowLess}
            onShowMore={byChannel.onShowMore}
            unread={entry.unread}
            unreadOverflow={entry.unreadOverflow}
          />
        ),
      };
    });
    rows = [
      ...grouped.slice(0, pinnedEntries),
      ...localPinned,
      ...grouped.slice(pinnedEntries),
      ...localRest,
    ];
  } else {
    const isPinnedRow = (row: ChatListRow) =>
      row.type === "room"
        ? isPinnedRoom(row.channel_id)
        : isPinnedThread(row.root_id ?? "");
    rows = [
      ...listed.filter(isPinnedRow).map(toSectionRow),
      ...localPinned,
      ...listed.filter((row) => !isPinnedRow(row)).map(toSectionRow),
      ...localRest,
    ];
  }
  return { counts, rows, unreadIds };
}

/** A room the relay did not send: its time only, no message line. */
function localPreview(channel: Channel): ChatRowPreview {
  const at = channel.lastMessageAt ? Date.parse(channel.lastMessageAt) : NaN;
  return { line: null, at: Number.isFinite(at) ? Math.floor(at / 1000) : null };
}

/**
 * The selected community's joined channels and visible DMs in one chat list.
 * With `serverChats` (the relay keeps chat state) rows, followed threads and
 * unread counts come from the relay; without it, this is today's list.
 */
export function ChatList({
  serverChats,
  view = "all",
  ...props
}: ChatListProps) {
  const { isBlocked } = useBlockedPeople();
  const items = useMemo(
    () =>
      sortChatList(props.items.filter(isListedChat), props.starredChannelIds),
    [props.items, props.starredChannelIds],
  );
  // "+ 5 more" clicks per channel, for this session only.
  const [expandedSteps, setExpandedSteps] = useState<
    ReadonlyMap<string, number>
  >(() => new Map());
  const onShowMore = useCallback((channelId: string) => {
    setExpandedSteps((current) =>
      new Map(current).set(channelId, (current.get(channelId) ?? 0) + 1),
    );
  }, []);
  const onShowLess = useCallback((channelId: string) => {
    setExpandedSteps((current) => {
      const next = new Map(current);
      next.delete(channelId);
      return next;
    });
  }, []);
  const byChannel = useMemo(
    () =>
      view === "by-channel" ? { expandedSteps, onShowLess, onShowMore } : null,
    [expandedSteps, onShowLess, onShowMore, view],
  );
  const working = useMemo(
    () =>
      splitChatWorking(props.activeWorkingByChannelId, serverChats?.rows ?? []),
    [props.activeWorkingByChannelId, serverChats?.rows],
  );
  // biome-ignore lint/correctness/useExhaustiveDependencies: readStateVersion signals that local read positions moved
  const server = useMemo(
    () =>
      serverChats
        ? buildServerChatRows(
            serverChats,
            items,
            props,
            byChannel,
            working.threads,
            isBlocked,
          )
        : null,
    [
      byChannel,
      isBlocked,
      working.threads,
      props.isActiveChannel,
      props.selectedChannelId,
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
      activeWorkingByChannelId={working.rooms}
      // With a thread open, its row is the selected one, not its channel's.
      selectedChannelId={
        serverChats?.openRootId ? null : props.selectedChannelId
      }
      items={items}
      rows={server.rows}
      title="Chats"
      testId="chat-list"
      unreadChannelCounts={server.counts}
      unreadChannelIds={server.unreadIds}
    />
  );
}
