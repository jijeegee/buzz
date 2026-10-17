import type { QueryClient } from "@tanstack/react-query";

import {
  applyLiveEventToChatList,
  CHAT_LIST_QUERY_KEY,
  createEventIdLru,
  type ChatListEvent,
  type ChatListState,
} from "@/features/chats/chatListState";
import {
  createTrailingDebounce,
  type TrailingDebounce,
} from "@/shared/lib/trailingDebounce";

/** A burst of list changes only the relay can resolve: one refetch. */
export const CHAT_LIST_INVALIDATE_DEBOUNCE_MS = 300;
/** Refetches for rows the list is missing run at most this often. */
export const CHAT_LIST_MISSING_ROW_THROTTLE_MS = 10_000;
const SEEN_EVENT_IDS = 512;

/** `replay` is set when re-applied over a list fetched meanwhile. */
export type ChatListUpdater = (
  state: ChatListState,
  replay: boolean,
) => ChatListState;

const journals = new WeakMap<QueryClient, Set<ChatListUpdater[]>>();

function openJournals(queryClient: QueryClient) {
  let set = journals.get(queryClient);
  if (!set) {
    set = new Set();
    journals.set(queryClient, set);
  }
  return set;
}

/**
 * Record every list patch made while a first page is fetched. The fetched
 * page predates them, so the caller re-applies them before storing it:
 * a patch is never lost to a fetch that started before it.
 */
export function recordChatListPatches(
  queryClient: QueryClient,
): () => ChatListUpdater[] {
  const journal: ChatListUpdater[] = [];
  const set = openJournals(queryClient);
  set.add(journal);
  return () => {
    set.delete(journal);
    return journal;
  };
}

/** Re-apply recorded patches to a freshly fetched list. */
export function replayChatListPatches(
  state: ChatListState,
  updaters: readonly ChatListUpdater[],
): ChatListState {
  return updaters.reduce((current, updater) => updater(current, true), state);
}

/**
 * Patch the shown chats list in place (and any list being fetched, once it
 * lands). Lists of other accounts are only marked stale.
 */
export function patchChatLists(
  queryClient: QueryClient,
  updater: ChatListUpdater,
): void {
  for (const journal of openJournals(queryClient)) journal.push(updater);
  queryClient.setQueriesData<ChatListState>(
    { queryKey: CHAT_LIST_QUERY_KEY, type: "active" },
    (current) => (current ? updater(current, false) : current),
  );
  void queryClient.invalidateQueries({
    queryKey: CHAT_LIST_QUERY_KEY,
    refetchType: "none",
    type: "inactive",
  });
}

type TimerHost = {
  setTimeout: (handler: () => void, ms: number) => number;
  clearTimeout: (id: number) => void;
};

/**
 * Live relay events → the server chats list: in-place patches, deduped by
 * event id, plus refetches only for rows the relay alone can place (a new
 * room, a thread just joined, a close or reopen), throttled.
 */
export class ChatListLiveSync {
  private readonly queryClient: QueryClient;
  private readonly isRoomHidden: (channelId: string) => boolean;
  private readonly now: () => number;
  private readonly timers: TimerHost;
  private readonly seen = createEventIdLru(SEEN_EVENT_IDS);
  private readonly debounce: TrailingDebounce;
  private lastMissingRowRefetch = Number.NEGATIVE_INFINITY;
  private missingRowTimer: number | null = null;

  constructor(
    queryClient: QueryClient,
    options: {
      /** Hidden DMs and archived rooms are not listed: never refetch for them. */
      isRoomHidden?: (channelId: string) => boolean;
      now?: () => number;
      timers?: TimerHost;
    } = {},
  ) {
    this.queryClient = queryClient;
    this.isRoomHidden = options.isRoomHidden ?? (() => false);
    this.now = options.now ?? Date.now;
    this.timers = options.timers ?? window;
    this.debounce = createTrailingDebounce(
      () => this.refetchNow(),
      CHAT_LIST_INVALIDATE_DEBOUNCE_MS,
      this.timers,
    );
  }

  hasLists(): boolean {
    return (
      this.queryClient.getQueriesData({ queryKey: CHAT_LIST_QUERY_KEY })
        .length > 0
    );
  }

  /** Reload the first page now (reconnect: events may have been missed). */
  refetchNow(): void {
    void this.queryClient.invalidateQueries({ queryKey: CHAT_LIST_QUERY_KEY });
  }

  /** Reload the first page after a quiet moment. */
  refetchSoon(): void {
    this.debounce.trigger();
  }

  /** A row the list should have is missing: at most one refetch per window. */
  refetchForMissingRow(channelId: string): void {
    if (!this.hasLists() || this.isRoomHidden(channelId)) return;
    if (this.missingRowTimer !== null) return;
    const wait =
      this.lastMissingRowRefetch +
      CHAT_LIST_MISSING_ROW_THROTTLE_MS -
      this.now();
    const run = () => {
      this.missingRowTimer = null;
      this.lastMissingRowRefetch = this.now();
      this.refetchSoon();
    };
    if (wait <= 0) run();
    else this.missingRowTimer = this.timers.setTimeout(run, wait);
  }

  /** A deletion: refetch only when a loaded row shows a deleted event. */
  onDeletion(deletedIds: readonly string[]): void {
    const deleted = new Set(deletedIds);
    const shown = this.queryClient
      .getQueriesData<ChatListState>({ queryKey: CHAT_LIST_QUERY_KEY })
      .some(([, list]) =>
        list?.rows.some(
          (row) =>
            (row.latest !== null && deleted.has(row.latest.id)) ||
            (row.root_id !== null && deleted.has(row.root_id)),
        ),
      );
    if (shown) this.refetchSoon();
  }

  /** One live message in a room this device lists. */
  onMessage(
    event: ChatListEvent,
    channelId: string,
    currentPubkey: string,
    mentionsMe: boolean,
  ): void {
    if (!this.hasLists() || this.seen.seen(event.id)) return;
    let missingRow = false;
    patchChatLists(this.queryClient, (current, replay) => {
      const patch = applyLiveEventToChatList(
        current,
        event,
        channelId,
        currentPubkey,
        mentionsMe,
        replay,
      );
      if (!replay) missingRow ||= patch.refetch;
      return patch.state;
    });
    if (missingRow) this.refetchForMissingRow(channelId);
  }

  dispose(): void {
    this.debounce.cancel();
    if (this.missingRowTimer !== null) {
      this.timers.clearTimeout(this.missingRowTimer);
      this.missingRowTimer = null;
    }
  }
}
