import type { ChatListRow, ChatRowUnread } from "./chatListState";

/** Live threads shown under a channel before "+ N more threads". */
export const BY_CHANNEL_VISIBLE_THREADS = 3;

/** Threads each "+ 10 more" click reveals under a channel. */
export const BY_CHANNEL_THREAD_STEP = 10;

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
      /** Threads still hidden; 0 once every thread shows. */
      count: number;
      /** Shows threads past the first ones, so "Show less" is offered. */
      expanded: boolean;
      unread: number;
      unreadOverflow: boolean;
      mentions: number;
      /** A hidden thread is marked unread with nothing to count. */
      marked: boolean;
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
 * that sums the hidden threads' unread and mentions. Each step in
 * `expandedSteps` shows `BY_CHANNEL_THREAD_STEP` more threads; the line then
 * counts only the threads still hidden and offers "Show less". A group with a
 * pinned room or thread goes to the top. `rows` comes in list order
 * (`orderChatRows`: pinned, then newest activity), so groups keep the order
 * of their first row. DMs group the same way: one row plus its threads.
 * `pinnedEntries` counts the leading entries that belong to pinned groups.
 */
export function groupChatRowsByChannel(
  rows: readonly ChatListRow[],
  {
    expandedSteps,
    isPinnedRoom,
    isPinnedThread,
    unreadOf,
    visibleThreads = BY_CHANNEL_VISIBLE_THREADS,
  }: {
    /** Channel id to "+ 10 more" clicks so far, for this session. */
    expandedSteps: ReadonlyMap<string, number>;
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
      steps: expandedSteps.get(group.channelId) ?? 0,
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
    steps,
    isPinnedThread,
    unreadOf,
    visibleThreads,
  }: {
    steps: number;
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
  if (threads.length <= limit) {
    for (const row of threads) entries.push({ kind: "row", row });
    return;
  }
  const shownCount = Math.min(
    threads.length,
    limit + steps * BY_CHANNEL_THREAD_STEP,
  );
  for (const row of threads.slice(0, shownCount)) {
    entries.push({ kind: "row", row });
  }
  let unread = 0;
  let unreadOverflow = false;
  let mentions = 0;
  let marked = false;
  for (const row of threads.slice(shownCount)) {
    const counts = unreadOf(row);
    unread += counts.count;
    unreadOverflow ||= counts.overflow;
    mentions += counts.mentions;
    marked ||= counts.marked;
  }
  entries.push({
    kind: "more",
    channelId: group.channelId,
    count: threads.length - shownCount,
    expanded: shownCount > limit,
    unread,
    unreadOverflow,
    mentions,
    marked,
  });
}
