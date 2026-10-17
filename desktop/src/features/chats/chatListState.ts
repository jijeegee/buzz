import {
  getThreadReference,
  isBroadcastReply,
} from "@/features/messages/lib/threading";
import type { RelayEvent } from "@/shared/api/types";
import {
  KIND_FORUM_COMMENT,
  KIND_FORUM_POST,
  KIND_STREAM_MESSAGE,
  KIND_STREAM_MESSAGE_V2,
} from "@/shared/constants/kinds";

/** Prefix of every server chat-list query; the full key adds relay and pubkey. */
export const CHAT_LIST_QUERY_KEY = ["chat-list"] as const;
export const CHAT_LIST_PAGE_SIZE = 100;
/** Rows loaded in the background after the first page. */
export const CHAT_LIST_BACKGROUND_ROWS = 400;
/** The relay caps unread and mention counts here and flags the overflow. */
export const CHAT_UNREAD_CAP = 99;

/** Message kinds the relay counts (kuzz-db `MESSAGE_KINDS`). */
const MESSAGE_KINDS: ReadonlySet<number> = new Set([
  KIND_STREAM_MESSAGE,
  KIND_STREAM_MESSAGE_V2,
  KIND_FORUM_POST,
  KIND_FORUM_COMMENT,
]);
/** Close / reopen control replies: they change which threads are listed. */
const THREAD_MARKER_TAGS: ReadonlySet<string> = new Set([
  "kuzz:thread-closed",
  "kuzz:thread-reopened",
]);

export type ChatListEvent = Pick<
  RelayEvent,
  "id" | "pubkey" | "created_at" | "kind" | "tags" | "content"
>;

/** One relay `chat_list` row: a room main timeline or one live thread. */
export type ChatListRow = {
  type: "room" | "thread";
  /** Channel UUID for rooms, thread root id for threads. */
  key: string;
  at: number;
  at_us: number;
  channel_id: string;
  channel_type: string;
  channel_name: string;
  archived: boolean;
  dm_members: string[];
  root_id: string | null;
  thread_name: string | null;
  follow_state: string | null;
  closed: boolean;
  last_seq: number | null;
  read_seq: number | null;
  unread: number;
  unread_overflow: boolean;
  unread_mentions: number;
  unread_mentions_overflow: boolean;
  unread_mark: boolean;
  latest: ChatListEvent | null;
  root: ChatListEvent | null;
};

export type ChatListCursor = { at_us: number; key: string };

export type ChatListPage = {
  rows: ChatListRow[];
  next: ChatListCursor | null;
};

export type ChatListState = ChatListPage & {
  /** Bumped by every fresh first page; fences late background pages. */
  generation: number;
};

export function chatListQueryKey(
  relayUrl: string,
  pubkey: string,
  pinnedThreadIds: readonly string[],
) {
  return [
    ...CHAT_LIST_QUERY_KEY,
    relayUrl,
    pubkey.toLowerCase(),
    pinnedThreadIds.join(","),
  ] as const;
}

/** Two chat list keys for the same relay and account (pins may differ). */
export function isSameChatListAccount(
  left: readonly unknown[],
  right: readonly unknown[],
): boolean {
  return (
    left.length === right.length &&
    left.slice(0, -1).every((part, index) => part === right[index])
  );
}

/** The ReadStateManager context a row's read position lives in. */
export function chatRowContextId(row: ChatListRow): string {
  return row.type === "thread" && row.root_id
    ? `thread:${row.root_id}`
    : row.channel_id;
}

function rowSortDesc(left: ChatListRow, right: ChatListRow) {
  return right.at_us - left.at_us || right.key.localeCompare(left.key);
}

/** Append a later page, keeping the first copy of any row. */
export function mergeChatListPage(
  current: ChatListState,
  page: ChatListPage,
): ChatListState {
  const keys = new Set(current.rows.map((row) => row.key));
  return {
    ...current,
    next: page.next,
    rows: [...current.rows, ...page.rows.filter((row) => !keys.has(row.key))],
  };
}

/**
 * A refetched first page over the loaded list: rows past the new first page
 * stay (with their cursor), so a refetch reloads one page, not all of them.
 */
export function mergeRefetchedFirstPage(
  previous: ChatListState | undefined,
  page: ChatListPage,
  generation: number,
): ChatListState {
  const last = page.rows.at(-1);
  if (!previous || page.next === null || !last) {
    return { ...page, generation };
  }
  const keys = new Set(page.rows.map((row) => row.key));
  const older = previous.rows.filter(
    (row) => !keys.has(row.key) && rowSortDesc(last, row) < 0,
  );
  if (older.length === 0) return { ...page, generation };
  return {
    generation,
    next: previous.next,
    rows: [...page.rows, ...older],
  };
}

function patchRow(
  current: ChatListState,
  matches: (row: ChatListRow) => boolean,
  patch: (row: ChatListRow) => ChatListRow | null,
): ChatListState {
  let changed = false;
  const rows: ChatListRow[] = [];
  for (const row of current.rows) {
    if (!matches(row)) {
      rows.push(row);
      continue;
    }
    const next = patch(row);
    changed ||= next !== row;
    if (next) rows.push(next);
  }
  return changed ? { ...current, rows } : current;
}

type MarkReadResultLike = {
  channel?: unknown;
  thread?: unknown;
  read_seq?: unknown;
  unread_mark?: unknown;
  stored?: unknown;
};

/**
 * Patch rows from this device's own `mark_read` response (read position and
 * unread mark as stored), instead of refetching on the ping it causes.
 */
export function applyMarkReadResponseToChatList(
  current: ChatListState,
  response: unknown,
): ChatListState {
  const items = (response as { items?: unknown } | null)?.items;
  if (!Array.isArray(items)) return current;
  let state = current;
  for (const raw of items as MarkReadResultLike[]) {
    if (raw?.stored !== true) continue;
    const readSeq = typeof raw.read_seq === "number" ? raw.read_seq : null;
    const unreadMark = raw.unread_mark === true;
    const channel = typeof raw.channel === "string" ? raw.channel : null;
    const thread = typeof raw.thread === "string" ? raw.thread : null;
    if (!channel && !thread) continue;
    state = patchRow(
      state,
      (row) =>
        thread
          ? row.type === "thread" && row.root_id === thread
          : row.type === "room" && row.channel_id === channel,
      (row) => {
        const caughtUp =
          readSeq !== null && row.last_seq !== null && readSeq >= row.last_seq;
        const next: ChatListRow = {
          ...row,
          read_seq: readSeq ?? row.read_seq,
          unread_mark: unreadMark,
          ...(caughtUp
            ? {
                unread: 0,
                unread_overflow: false,
                unread_mentions: 0,
                unread_mentions_overflow: false,
              }
            : {}),
        };
        return next;
      },
    );
  }
  return state;
}

/**
 * Show this device's own "Mark as unread" on a row before the relay answers;
 * the queued write's response then patches the same row again.
 */
export function applyUnreadMarkToChatList(
  current: ChatListState,
  target: { thread: string } | { channel: string },
): ChatListState {
  return patchRow(
    current,
    (row) =>
      "thread" in target
        ? row.type === "thread" && row.root_id === target.thread
        : row.type === "room" && row.channel_id === target.channel,
    (row) => (row.unread_mark ? row : { ...row, unread_mark: true }),
  );
}

/**
 * Patch rows from this device's own `thread_follow` response. Unfollowing
 * drops the row; a newly followed thread not loaded yet needs the relay.
 */
export function applyFollowResponseToChatList(
  current: ChatListState,
  response: unknown,
): ChatListLivePatch {
  const { root, state } = (response ?? {}) as {
    root?: unknown;
    state?: unknown;
  };
  if (typeof root !== "string") return { state: current, refetch: false };
  const loaded = current.rows.some(
    (row) => row.type === "thread" && row.root_id === root,
  );
  if (state === "unfollowed") {
    return {
      state: patchRow(
        current,
        (row) => row.type === "thread" && row.root_id === root,
        () => null,
      ),
      refetch: false,
    };
  }
  if (state !== "followed") return { state: current, refetch: false };
  if (!loaded) return { state: current, refetch: true };
  return {
    state: patchRow(
      current,
      (row) => row.type === "thread" && row.root_id === root,
      (row) =>
        row.follow_state === "followed"
          ? row
          : { ...row, follow_state: "followed" },
    ),
    refetch: false,
  };
}

/**
 * Remembers the last `limit` event ids: `seen(id)` is true for a repeat
 * (reconnect replay, a second subscription) and records the id otherwise.
 */
export function createEventIdLru(limit: number) {
  const ids = new Set<string>();
  return {
    seen(id: string): boolean {
      if (ids.has(id)) return true;
      ids.add(id);
      if (ids.size > limit) {
        const oldest = ids.values().next().value;
        if (oldest !== undefined) ids.delete(oldest);
      }
      return false;
    },
  };
}

export type ChatListLivePatch = {
  state: ChatListState;
  /** The event may add a row only the relay can place: refetch the list. */
  refetch: boolean;
};

function bump(count: number, overflow: boolean) {
  return count >= CHAT_UNREAD_CAP
    ? { count: CHAT_UNREAD_CAP, overflow: true }
    : { count: count + 1, overflow };
}

function hasThreadMarker(event: ChatListEvent, rootId: string): boolean {
  return event.tags.some(
    (tag) =>
      THREAD_MARKER_TAGS.has(tag[0]) &&
      tag[1]?.toLowerCase() === rootId.toLowerCase(),
  );
}

/**
 * Fold one live message into the loaded chats list, as the relay's write path
 * does: a top-level (or broadcast) message moves its room row, a reply moves
 * its thread row; others' messages add unread (and mentions), your own marks
 * the row read. Rows the relay would add (a new room, a thread you just
 * joined by replying or being mentioned, a close or reopen) ask for a refetch.
 */
export function applyLiveEventToChatList(
  current: ChatListState,
  event: ChatListEvent,
  channelId: string,
  currentPubkey: string,
  mentionsMe: boolean,
  /**
   * Re-applying an event that arrived while this list was fetched: the
   * fetched rows may already count it, so only strictly newer events apply.
   */
  replay = false,
): ChatListLivePatch {
  const unchanged = { state: current, refetch: false };
  if (!MESSAGE_KINDS.has(event.kind)) return unchanged;
  const { rootId } = getThreadReference(event.tags);
  if (rootId && hasThreadMarker(event, rootId)) {
    return { state: current, refetch: true };
  }
  const me = currentPubkey.toLowerCase();
  const fromMe = me.length > 0 && event.pubkey.toLowerCase() === me;
  const targets: Array<{ type: "room" | "thread"; rootId: string | null }> =
    rootId
      ? [
          { type: "thread", rootId },
          ...(isBroadcastReply(event.tags)
            ? [{ type: "room" as const, rootId: null }]
            : []),
        ]
      : [{ type: "room", rootId: null }];

  let rows = current.rows;
  let refetch = false;
  for (const target of targets) {
    const index = rows.findIndex((row) =>
      target.type === "thread"
        ? row.type === "thread" && row.root_id === target.rootId
        : row.type === "room" && row.channel_id === channelId,
    );
    if (index === -1) {
      // An unknown room is new or beyond the loaded pages; an unknown thread
      // is listed only once you take part in it.
      refetch ||= target.type === "room" || fromMe || mentionsMe;
      continue;
    }
    const existing = rows[index];
    if (existing.latest?.id === event.id) continue;
    if (
      existing.latest &&
      (replay
        ? existing.latest.created_at >= event.created_at
        : existing.latest.created_at > event.created_at)
    ) {
      continue;
    }
    const unread = fromMe
      ? { count: 0, overflow: false }
      : bump(existing.unread, existing.unread_overflow);
    const mentions = fromMe
      ? { count: 0, overflow: false }
      : mentionsMe
        ? bump(existing.unread_mentions, existing.unread_mentions_overflow)
        : {
            count: existing.unread_mentions,
            overflow: existing.unread_mentions_overflow,
          };
    const next: ChatListRow = {
      ...existing,
      at: Math.max(existing.at, event.created_at),
      at_us: Math.max(existing.at_us, event.created_at * 1_000_000),
      latest: event,
      unread: unread.count,
      unread_overflow: unread.overflow,
      unread_mentions: mentions.count,
      unread_mentions_overflow: mentions.overflow,
      unread_mark: fromMe ? false : existing.unread_mark,
    };
    rows = rows.map((row, rowIndex) => (rowIndex === index ? next : row));
  }
  if (rows === current.rows) return { state: current, refetch };
  return { state: { ...current, rows: [...rows].sort(rowSortDesc) }, refetch };
}

/** A row's unread state as shown, after this device's own read position. */
export type ChatRowUnread = {
  count: number;
  overflow: boolean;
  mentions: number;
  mentionsOverflow: boolean;
  /** Marked unread with nothing to count. */
  marked: boolean;
};

/**
 * Server counts, unless this device has already read past the row's latest
 * message (the mark-read mirror has not reached the relay yet). A row the
 * user marked unread stays marked.
 */
export function chatRowUnread(
  row: ChatListRow,
  ownReadAt: number | null,
): ChatRowUnread {
  const readLocally =
    ownReadAt !== null &&
    row.latest !== null &&
    ownReadAt >= row.latest.created_at &&
    !row.unread_mark;
  if (readLocally) {
    return {
      count: 0,
      overflow: false,
      mentions: 0,
      mentionsOverflow: false,
      marked: false,
    };
  }
  return {
    count: row.unread,
    overflow: row.unread_overflow,
    mentions: row.unread_mentions,
    mentionsOverflow: row.unread_mentions_overflow,
    marked: row.unread_mark && row.unread === 0,
  };
}

/** "99+" past the relay's cap. */
export function formatChatCount(count: number, overflow: boolean): string {
  return overflow || count > CHAT_UNREAD_CAP
    ? `${CHAT_UNREAD_CAP}+`
    : String(count);
}

/**
 * Pinned rows first (pinned rooms by star, pinned threads by id), each group
 * newest first as the relay sent them; then everything else by activity.
 */
export function orderChatRows(
  rows: readonly ChatListRow[],
  pinnedRoomIds: ReadonlySet<string> | undefined,
  pinnedThreadIds: ReadonlySet<string> | undefined,
): ChatListRow[] {
  const isPinned = (row: ChatListRow) =>
    row.type === "room"
      ? (pinnedRoomIds?.has(row.channel_id) ?? false)
      : row.root_id !== null && (pinnedThreadIds?.has(row.root_id) ?? false);
  const sorted = [...rows].sort(rowSortDesc);
  return [
    ...sorted.filter(isPinned),
    ...sorted.filter((row) => !isPinned(row)),
  ];
}

/** A thread row's title: its name, else the root's first line, trimmed. */
export function chatThreadTitle(row: ChatListRow): string {
  const name = row.thread_name?.trim();
  if (name) return name;
  const firstLine = row.root?.content
    .split("\n")
    .map((line) => line.trim())
    .find((line) => line.length > 0);
  if (!firstLine) return "Thread";
  return firstLine.length > 80 ? `${firstLine.slice(0, 80)}…` : firstLine;
}
