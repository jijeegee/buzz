import {
  getThreadReference,
  isBroadcastReply,
} from "@/features/messages/lib/threading";
import type { InboxItem } from "@/features/home/lib/inbox";
import type { Channel, FeedItem, RelayEvent } from "@/shared/api/types";
import {
  KIND_STREAM_MESSAGE,
  KIND_STREAM_MESSAGE_V2,
} from "@/shared/constants/kinds";

export const INBOX_SUMMARY_QUERY_KEY = ["inbox-summary"] as const;
/** Rows loaded in the background after the first page. */
export const INBOX_SUMMARY_BACKGROUND_ROWS = 300;
export const INBOX_SUMMARY_PAGE_SIZE = 30;

/** The chat list shows chat messages only, as the per-room loading did. */
const SUMMARY_KINDS: ReadonlySet<number> = new Set([
  KIND_STREAM_MESSAGE,
  KIND_STREAM_MESSAGE_V2,
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
  /** The read markers the counts were computed against. */
  sentRead: Readonly<Record<string, number>>;
};

/** A row's server count and the marker and message time it was taken at. */
export type InboxServerUnread = {
  count: number;
  /** created_at of the row's latest message (not the relay receive time). */
  latestAt: number;
  /** Effective read marker sent for this row, or null. */
  sentReadAt: number | null;
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
    for (const event of [row.root, row.latest]) {
      if (event && SUMMARY_KINDS.has(event.kind)) {
        items.push(toFeedItem(event, row, channel));
      }
    }
  }
  return items;
}

/** Server unread counts keyed by Inbox conversation id, with the row's activity time. */
export function inboxSummaryUnread(
  rows: readonly InboxSummaryRow[],
  sentRead: Readonly<Record<string, number>>,
) {
  const unread = new Map<string, InboxServerUnread>();
  for (const row of rows) {
    if (row.unread === null) continue;
    const id = inboxSummaryConversationId(row);
    const previous = unread.get(id);
    const channelRead = sentRead[row.channel_id] ?? null;
    const threadRead =
      row.type === "thread" && row.channel_type !== "dm" && row.root_id
        ? (sentRead[`thread:${row.root_id}`] ?? null)
        : null;
    const sentReadAt =
      threadRead === null
        ? channelRead
        : Math.max(threadRead, channelRead ?? 0);
    // DM threads fold into the DM row, so their counts add up.
    unread.set(id, {
      count: (previous?.count ?? 0) + row.unread,
      latestAt: Math.max(
        previous?.latestAt ?? 0,
        row.latest?.created_at ?? row.at,
      ),
      sentReadAt: previous?.sentReadAt ?? sentReadAt,
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
    // Relay `at` is a receive time; compare message times only.
    if (existing?.latest && existing.latest.created_at > event.created_at) {
      continue;
    }
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
  unread: ReadonlyMap<string, InboxServerUnread>,
  readAt: (row: T) => number | null,
  isSettled: (row: T) => boolean,
): T[] {
  return rows.map((row) => {
    const server = unread.get(row.conversationId);
    // Read (including per-message reads), muted, or a marker that moved since
    // the summary was counted: the local count is the current truth.
    if (!server || isSettled(row)) return row;
    const marker = readAt(row);
    if (marker !== server.sentReadAt) return row;
    if (marker !== null && marker >= server.latestAt) return row;
    const count = Math.max(row.unreadCount, server.count);
    return count === row.unreadCount ? row : { ...row, unreadCount: count };
  });
}

/** Event ids a deletion event targets (`e` tags). */
export function deletedEventIds(event: Pick<RelayEvent, "tags">): string[] {
  return event.tags
    .filter((tag) => tag[0] === "e" && typeof tag[1] === "string")
    .map((tag) => tag[1]);
}

/** True when any id is a row's shown latest message or thread root. */
export function inboxSummaryShowsAny(
  state: InboxSummaryState,
  ids: readonly string[],
): boolean {
  if (ids.length === 0) return false;
  const wanted = new Set(ids);
  return state.rows.some(
    (row) =>
      (row.latest !== null && wanted.has(row.latest.id)) ||
      (row.root_id !== null && wanted.has(row.root_id)),
  );
}
