import { invokeTauri } from "@/shared/api/tauri";
import type {
  ChatListCursor,
  ChatListPage,
  ChatListRow,
} from "@/features/chats/chatListState";

/** Error text the Tauri commands use when the relay keeps no chat state. */
export const CHAT_STATE_UNAVAILABLE = "chat_state_unavailable";
/**
 * Error text when the active relay or signer is no longer the account a
 * request was queued for. Nothing was sent; the request must be dropped.
 */
export const CHAT_STATE_ACCOUNT_CHANGED = "chat_state_account_changed";

/**
 * The account a chat-state request belongs to, captured when it was queued.
 * The backend refuses the request when either no longer matches.
 */
export type ChatStateBinding = { relayUrl: string; pubkey: string };

/**
 * One room main timeline or thread, read up to `at` (unix seconds). Without
 * `at` only the unread mark changes.
 */
export type ChatMarkReadItem = {
  kind: "channel" | "thread";
  /** Channel UUID, or the thread root event id. */
  id: string;
  at?: number;
};

export type ChatFollowState = "followed" | "unfollowed";

/** One stored item from a `mark_read` response. */
export type ChatMarkReadResult = {
  channel?: string;
  thread?: string;
  channel_id: string;
  read_seq: number | null;
  unread_mark: boolean;
  stored: boolean;
};

export type ChatMarkReadResponse = {
  mark_read: 1;
  items: ChatMarkReadResult[];
};

export type ChatThreadFollowResponse = {
  thread_follow: 1;
  root: string;
  channel_id: string;
  state: ChatFollowState;
};

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** True when the relay has the chat-state switch off or has no chat state at all. */
export function isChatStateUnavailable(error: unknown): boolean {
  return errorText(error).includes(CHAT_STATE_UNAVAILABLE);
}

/** True when the request was refused because the account or relay changed. */
export function isChatStateAccountChanged(error: unknown): boolean {
  return errorText(error).includes(CHAT_STATE_ACCOUNT_CHANGED);
}

/**
 * Index of the `mark_read` item the relay refused (`item N: ...` with a 403 or
 * 404), or null. All items of a refused request are left unchanged.
 */
export function refusedMarkReadItem(error: unknown): number | null {
  const match = /relay returned 40[34][^:]*: mark_read item (\d+):/.exec(
    errorText(error),
  );
  return match ? Number(match[1]) : null;
}

/** A request the relay rejected as malformed or forbidden; retrying cannot help. */
export function isPermanentChatStateError(error: unknown): boolean {
  // Rate limits read "relay rate-limited", not "relay returned 429".
  const status = /relay returned (4\d\d)/.exec(errorText(error))?.[1];
  return status !== undefined && status !== "408";
}

/** One page of the user's server-side chats list. */
export function getChatListPage(
  binding: ChatStateBinding,
  before: ChatListCursor | null,
  pinnedThreads: readonly string[],
  limit: number,
): Promise<ChatListPage> {
  return invokeTauri<ChatListPage>("get_chat_list", {
    before,
    binding,
    limit,
    pinnedThreads,
  });
}

/** Most threads one name search returns. */
export const THREAD_NAME_SEARCH_LIMIT = 20;

/**
 * A `thread_search` row: the `chat_list` thread row fields without counts
 * or events, plus how well the name matched (0 exact .. 3 middle).
 */
export type ThreadNameSearchRow = Pick<
  ChatListRow,
  | "type"
  | "key"
  | "at"
  | "at_us"
  | "channel_id"
  | "channel_type"
  | "channel_name"
  | "archived"
  | "dm_members"
  | "group_chat"
  | "member_count"
  | "root_id"
  | "thread_name"
  | "closed"
> & { match: number };

/**
 * Threads in the user's rooms whose name contains `query`, closed and quiet
 * ones included (the Chats list leaves those out). Best match first.
 */
export async function searchThreadNames(
  binding: ChatStateBinding,
  query: string,
  limit = THREAD_NAME_SEARCH_LIMIT,
): Promise<ThreadNameSearchRow[]> {
  const response = await invokeTauri<{ rows: ThreadNameSearchRow[] }>(
    "search_thread_names",
    { binding, limit, query },
  );
  return response.rows;
}

/**
 * Owner view: one page of `agent`'s DMs and group chats that the signed-in
 * owner is not in. Read-only; rows carry no read state.
 */
export function getAgentChatListPage(
  binding: ChatStateBinding,
  agent: string,
  before: ChatListCursor | null,
  limit: number,
): Promise<ChatListPage> {
  return invokeTauri<ChatListPage>("get_agent_chat_list", {
    agent,
    before,
    binding,
    limit,
  });
}

/**
 * Store read positions (the relay only moves them forward) and set every
 * item's unread mark to `unreadMark`.
 */
export function chatMarkRead(
  binding: ChatStateBinding,
  items: readonly ChatMarkReadItem[],
  unreadMark = false,
) {
  return invokeTauri<ChatMarkReadResponse>("chat_mark_read", {
    binding,
    items,
    unreadMark,
  });
}

/** Follow or unfollow a thread by root id. */
export function chatThreadFollow(
  binding: ChatStateBinding,
  root: string,
  followState: ChatFollowState,
) {
  return invokeTauri<ChatThreadFollowResponse>("chat_thread_follow", {
    binding,
    followState,
    root,
  });
}

/** One device-local read position for `chat_import` (unix seconds). */
export type ChatImportRead = {
  kind: "channel" | "thread";
  id: string;
  at: number;
};

/** One device-local follow choice for `chat_import`. */
export type ChatImportFollow = { root: string; state: ChatFollowState };

/** Why an imported item was skipped (`null` when it applied). */
export type ChatImportSkipReason =
  | "not_member"
  | "unknown_thread"
  | "not_thread_root"
  | "not_followed"
  | "already_set";

export type ChatImportItemResult = {
  applied: boolean;
  reason: ChatImportSkipReason | null;
  channel_id: string | null;
  read_seq?: number | null;
};

export type ChatImportResponse = {
  chat_import: 1;
  reads: ChatImportItemResult[];
  follows: ChatImportItemResult[];
};

/**
 * First-run upload of this device's read positions and follows (1..=500
 * items). The relay applies each only over its backfilled state.
 */
export function chatImport(
  binding: ChatStateBinding,
  reads: readonly ChatImportRead[],
  follows: readonly ChatImportFollow[],
) {
  return invokeTauri<ChatImportResponse>("chat_import", {
    binding,
    follows,
    reads,
  });
}
