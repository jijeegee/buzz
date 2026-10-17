import { invokeTauri } from "@/shared/api/tauri";
import type {
  ChatListCursor,
  ChatListPage,
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
