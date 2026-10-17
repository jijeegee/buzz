import type { ChatListRow, ChatRowUnread } from "./chatListState";

/** Live threads shown under a channel before "+ N more threads". */
export const BY_CHANNEL_VISIBLE_THREADS = 3;

export type ChatsViewMode = "all" | "by-channel";

const VIEW_STORAGE_KEY = "kuzz-chats-view.v1";

export function parseChatsViewMode(value: unknown): ChatsViewMode {
  return value === "by-channel" ? "by-channel" : "all";
}

/** This device's chosen Chats view (not synced). */
export function readChatsViewMode(
  storage: Pick<Storage, "getItem"> | undefined,
): ChatsViewMode {
  try {
    return parseChatsViewMode(storage?.getItem(VIEW_STORAGE_KEY));
  } catch {
    return "all";
  }
}

export function writeChatsViewMode(
  storage: Pick<Storage, "setItem"> | undefined,
  mode: ChatsViewMode,
) {
  try {
    storage?.setItem(VIEW_STORAGE_KEY, mode);
  } catch {
    // Storage full or blocked: the choice lasts for this session only.
  }
}

export type ChatGroupEntry =
  | { kind: "row"; row: ChatListRow }
  /** The channel's heading when the relay has not sent its main row yet. */
  | { kind: "channel"; channelId: string }
  | {
      kind: "more";
      channelId: string;
      /** Threads hidden (collapsed) or shown below the first ones (expanded). */
      count: number;
      expanded: boolean;
      unread: number;
      unreadOverflow: boolean;
      mentions: number;
    };

type Group = {
  channelId: string;
  pinned: boolean;
  room: ChatListRow | null;
  threads: ChatListRow[];
};

/**
 * View B, By channel: each channel's main row, then its newest live threads
 * (pinned ones first) up to `visibleThreads`, then a "+ N more threads" line
 * that sums the hidden threads' unread and mentions. Expanded groups list
 * every thread and end with the same line as "Show less". A group with a
 * pinned room or thread goes to the top. `rows` comes in list order
 * (`orderChatRows`: pinned, then newest activity), so groups keep the order
 * of their first row. DMs group the same way: one row plus its threads.
 * `pinnedEntries` counts the leading entries that belong to pinned groups.
 */
export function groupChatRowsByChannel(
  rows: readonly ChatListRow[],
  {
    expandedChannelIds,
    isPinnedRoom,
    isPinnedThread,
    unreadOf,
    visibleThreads = BY_CHANNEL_VISIBLE_THREADS,
  }: {
    expandedChannelIds: ReadonlySet<string>;
    isPinnedRoom: (channelId: string) => boolean;
    isPinnedThread: (rootId: string) => boolean;
    unreadOf: (row: ChatListRow) => ChatRowUnread;
    visibleThreads?: number;
  },
): { entries: ChatGroupEntry[]; pinnedEntries: number } {
  const groups = new Map<string, Group>();
  for (const row of rows) {
    let group = groups.get(row.channel_id);
    if (!group) {
      group = {
        channelId: row.channel_id,
        // A pinned room pins its group even before its main row loads.
        pinned: isPinnedRoom(row.channel_id),
        room: null,
        threads: [],
      };
      groups.set(row.channel_id, group);
    }
    if (row.type === "room") {
      group.room ??= row;
    } else if (row.root_id) {
      group.threads.push(row);
      group.pinned ||= isPinnedThread(row.root_id);
    }
  }
  const ordered = [...groups.values()];
  const sorted = [
    ...ordered.filter((group) => group.pinned),
    ...ordered.filter((group) => !group.pinned),
  ];
  const entries: ChatGroupEntry[] = [];
  let pinnedEntries = 0;
  for (const group of sorted) {
    appendGroup(entries, group, {
      expanded: expandedChannelIds.has(group.channelId),
      isPinnedThread,
      unreadOf,
      visibleThreads,
    });
    if (group.pinned) pinnedEntries = entries.length;
  }
  return { entries, pinnedEntries };
}

function appendGroup(
  entries: ChatGroupEntry[],
  group: Group,
  {
    expanded,
    isPinnedThread,
    unreadOf,
    visibleThreads,
  }: {
    expanded: boolean;
    isPinnedThread: (rootId: string) => boolean;
    unreadOf: (row: ChatListRow) => ChatRowUnread;
    visibleThreads: number;
  },
) {
  entries.push(
    group.room
      ? { kind: "row", row: group.room }
      : { kind: "channel", channelId: group.channelId },
  );
  const pinnedThreads = group.threads.filter((row) =>
    isPinnedThread(row.root_id ?? ""),
  );
  const threads = [
    ...pinnedThreads,
    ...group.threads.filter((row) => !pinnedThreads.includes(row)),
  ];
  // Pinned threads are always shown, even past the usual three.
  const limit = Math.max(visibleThreads, pinnedThreads.length);
  const shown = expanded ? threads : threads.slice(0, limit);
  for (const row of shown) entries.push({ kind: "row", row });
  if (threads.length <= limit) return;
  let unread = 0;
  let unreadOverflow = false;
  let mentions = 0;
  if (!expanded) {
    for (const row of threads.slice(limit)) {
      const counts = unreadOf(row);
      unread += counts.count;
      unreadOverflow ||= counts.overflow;
      mentions += counts.mentions;
    }
  }
  entries.push({
    kind: "more",
    channelId: group.channelId,
    count: threads.length - limit,
    expanded,
    unread,
    unreadOverflow,
    mentions,
  });
}
