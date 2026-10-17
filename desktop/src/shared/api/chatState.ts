import { invokeTauri } from "@/shared/api/tauri";
import type {
  ChatListCursor,
  ChatListPage,
} from "@/features/chats/chatListState";

/** Error text the Tauri commands use when the relay keeps no chat state. */
export const CHAT_STATE_UNAVAILABLE = "chat_state_unavailable";

/** One read position: a room main timeline or a thread, read up to `at` (unix seconds). */
export type ChatMarkReadItem = {
  kind: "channel" | "thread";
  /** Channel UUID, or the thread root event id. */
  id: string;
  at: number;
};

export type ChatFollowState = "followed" | "unfollowed";

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** True when the relay has the chat-state switch off or has no chat state at all. */
export function isChatStateUnavailable(error: unknown): boolean {
  return errorText(error).includes(CHAT_STATE_UNAVAILABLE);
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
  before: ChatListCursor | null,
  pinnedThreads: readonly string[],
  limit: number,
): Promise<ChatListPage> {
  return invokeTauri<ChatListPage>("get_chat_list", {
    before,
    limit,
    pinnedThreads,
  });
}

/** Store read positions; the relay only moves them forward. */
export function chatMarkRead(items: readonly ChatMarkReadItem[]) {
  return invokeTauri<unknown>("chat_mark_read", { items });
}

/** Follow or unfollow a thread by root id. */
export function chatThreadFollow(root: string, followState: ChatFollowState) {
  return invokeTauri<unknown>("chat_thread_follow", { followState, root });
}
