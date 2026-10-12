import {
  getThreadReference,
  isBroadcastReply,
} from "@/features/messages/lib/threading";
import type { InboxItem } from "@/features/home/lib/inbox";
import type { Channel, FeedItem, RelayEvent } from "@/shared/api/types";
import {
  KIND_FORUM_COMMENT,
  KIND_FORUM_POST,
  KIND_STREAM_MESSAGE,
  KIND_STREAM_MESSAGE_V2,
} from "@/shared/constants/kinds";

export const INBOX_SUMMARY_QUERY_KEY = ["inbox-summary"] as const;
/** Rows loaded in the background after the first page. */
export const INBOX_SUMMARY_BACKGROUND_ROWS = 300;
export const INBOX_SUMMARY_PAGE_SIZE = 30;

const SUMMARY_KINDS: ReadonlySet<number> = new Set([
  KIND_STREAM_MESSAGE,
  KIND_STREAM_MESSAGE_V2,
  KIND_FORUM_POST,
  KIND_FORUM_COMMENT,
]);

export type InboxSummaryEvent = Pick<
  RelayEvent,
  "id" | "pubkey" | "created_at" | "kind" | "tags" | "content"
>;

/** One relay `inbox_summary` row: a room main timeline or one thread. */
export type InboxSummaryRow = {
  type: "room" | "thread";
  channel_id: string;
  channel_type: string;
  root_id: string | null;
  at: number;
  at_us: number;
  key: string;
  reply_count: number | null;
  /** Unread messages from others after the sent marker; null when unknown. */
  unread: number | null;
  unread_overflow: boolean;
  latest: InboxSummaryEvent | null;
  root: InboxSummaryEvent | null;
};

export type InboxSummaryCursor = { at_us: number; key: string };

export type InboxSummaryPage = {
  rows: InboxSummaryRow[];
  next: InboxSummaryCursor | null;
};

export type InboxSummaryState = InboxSummaryPage & {
  /** Bumped by every fresh first page; fences late background pages. */
  generation: number;
  /** False when read state had not loaded yet, so counts are unknown. */
  sentReadMarkers: boolean;
};

/** The Inbox conversation id a summary row stands for in the chat-list view. */
export function inboxSummaryConversationId(row: InboxSummaryRow): string {
  if (row.channel_type === "dm") return `dm:${row.channel_id}`;
  return row.type === "thread" && row.root_id
    ? row.root_id
    : `channel:${row.channel_id}`;
}

function rowSortDesc(left: InboxSummaryRow, right: InboxSummaryRow) {
  return right.at_us - left.at_us || right.key.localeCompare(left.key);
}

/** Append a later page, keeping the first copy of any row. */
export function mergeInboxSummaryPage(
  current: InboxSummaryState,
  page: InboxSummaryPage,
): InboxSummaryState {
  const keys = new Set(current.rows.map((row) => row.key));
  return {
    ...current,
    next: page.next,
    rows: [...current.rows, ...page.rows.filter((row) => !keys.has(row.key))],
  };
}

function toFeedItem(
  event: InboxSummaryEvent,
  row: InboxSummaryRow,
  channel: Channel | undefined,
): FeedItem {
  return {
    category: "activity",
    channelId: row.channel_id,
    channelName: channel?.name ?? "",
    channelType: channel?.channelType ?? row.channel_type,
    content: event.content,
    createdAt: event.created_at,
    id: event.id,
    kind: event.kind,
    pubkey: event.pubkey,
    tags: event.tags,
  };
}

/** Feed items for the existing Inbox pipeline: each row's latest message, plus a thread's root. */
export function inboxSummaryFeedItems(
  rows: readonly InboxSummaryRow[],
  channelById: ReadonlyMap<string, Channel>,
): FeedItem[] {
  const items: FeedItem[] = [];
  for (const row of rows) {
    const channel = channelById.get(row.channel_id);
    if (row.root) items.push(toFeedItem(row.root, row, channel));
    if (row.latest) items.push(toFeedItem(row.latest, row, channel));
  }
  return items;
}

/** Server unread counts keyed by Inbox conversation id, with the row's activity time. */
export function inboxSummaryUnread(rows: readonly InboxSummaryRow[]) {
  const unread = new Map<string, { count: number; at: number }>();
  for (const row of rows) {
    if (row.unread === null) continue;
    const id = inboxSummaryConversationId(row);
    const previous = unread.get(id);
    // DM threads fold into the DM row, so their counts add up.
    unread.set(id, {
      count: (previous?.count ?? 0) + row.unread,
      at: Math.max(previous?.at ?? 0, row.at),
    });
  }
  return unread;
}

/**
 * Fold one live event into the loaded summary: only the room or thread row it
 * belongs to changes. Returns `current` unchanged for events that do not move
 * a row (other kinds, duplicates, events older than the row).
 */
export function applyLiveEventToInboxSummary(
  current: InboxSummaryState,
  event: InboxSummaryEvent,
  channel: { id: string; channelType: string },
  currentPubkey: string,
): InboxSummaryState {
  if (!SUMMARY_KINDS.has(event.kind)) return current;
  const { rootId } = getThreadReference(event.tags);
  const fromOther =
    currentPubkey.length > 0 &&
    event.pubkey.toLowerCase() !== currentPubkey.toLowerCase();
  const targets: Array<{ type: "room" | "thread"; rootId: string | null }> = [];
  if (rootId) {
    targets.push({ type: "thread", rootId });
    if (isBroadcastReply(event.tags))
      targets.push({ type: "room", rootId: null });
  } else {
    targets.push({ type: "room", rootId: null });
  }

  let rows = current.rows;
  for (const target of targets) {
    const index = rows.findIndex((row) =>
      target.type === "thread"
        ? row.type === "thread" && row.root_id === target.rootId
        : row.type === "room" && row.channel_id === channel.id,
    );
    const existing = index === -1 ? null : rows[index];
    if (existing?.latest?.id === event.id) continue;
    if (existing && existing.at > event.created_at) continue;
    const base: InboxSummaryRow = existing ?? {
      type: target.type,
      channel_id: channel.id,
      channel_type: channel.channelType,
      root_id: target.rootId,
      at: event.created_at,
      at_us: event.created_at * 1_000_000,
      key: target.rootId ?? channel.id,
      reply_count: target.type === "thread" ? 0 : null,
      unread: null,
      unread_overflow: false,
      latest: null,
      root: null,
    };
    const next: InboxSummaryRow = {
      ...base,
      at: event.created_at,
      at_us: Math.max(base.at_us, event.created_at * 1_000_000),
      latest: event,
      reply_count:
        target.type === "thread" ? (base.reply_count ?? 0) + 1 : null,
      unread: base.unread !== null && fromOther ? base.unread + 1 : base.unread,
    };
    rows =
      index === -1
        ? [...rows, next]
        : rows.map((row, rowIndex) => (rowIndex === index ? next : row));
  }
  if (rows === current.rows) return current;
  return { ...current, rows: [...rows].sort(rowSortDesc) };
}

type ReadAt = (contextId: string) => number | null;

/** The read marker that governs a chat-list row. */
export function inboxRowReadAt(
  row: Pick<InboxItem, "conversationId" | "item">,
  getChannelReadAt: ReadAt,
  getThreadReadAt: (rootId: string, channelId?: string | null) => number | null,
): number | null {
  const { conversationId } = row;
  for (const prefix of ["channel:", "dm:"]) {
    if (conversationId.startsWith(prefix)) {
      return getChannelReadAt(conversationId.slice(prefix.length));
    }
  }
  return getThreadReadAt(conversationId, row.item.channelId);
}

/**
 * Show the relay's unread count on chat-list rows. A row whose marker has
 * reached its latest activity is read, whatever the summary counted.
 */
export function withServerUnreadCounts<T extends InboxItem>(
  rows: readonly T[],
  unread: ReadonlyMap<string, { count: number; at: number }>,
  readAt: (row: T) => number | null,
): T[] {
  return rows.map((row) => {
    const server = unread.get(row.conversationId);
    if (!server) return row;
    const marker = readAt(row);
    const count =
      marker !== null && marker >= server.at
        ? 0
        : Math.max(row.unreadCount, server.count);
    return count === row.unreadCount ? row : { ...row, unreadCount: count };
  });
}
