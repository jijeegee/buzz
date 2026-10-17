import { useQuery, useQueryClient } from "@tanstack/react-query";
import * as React from "react";

import {
  CHAT_LIST_BACKGROUND_ROWS,
  CHAT_LIST_PAGE_SIZE,
  chatListQueryKey,
  mergeChatListPage,
  mergeRefetchedFirstPage,
  type ChatListState,
} from "@/features/chats/chatListState";
import {
  recordChatListPatches,
  replayChatListPatches,
} from "@/features/chats/chatListSync";
import { setChatStateCapability } from "@/features/chats/chatStateCapability";
import {
  getChatListPage,
  isChatStateUnavailable,
} from "@/shared/api/chatState";

let chatListGeneration = 0;
const NO_PINS: readonly string[] = [];

/**
 * The server chats list while the chat-state capability is on: the first
 * page renders at once, later pages load one after another in the background
 * up to CHAT_LIST_BACKGROUND_ROWS. A refetch (ping, reconnect) reloads only
 * the first page and keeps the older loaded rows. Live events patch rows in
 * place (chatListSync); patches made while a first page was in flight are
 * re-applied to it. An "unavailable" answer turns the capability off, which
 * disables this query.
 */
export function useServerChatList({
  capabilityKey,
  connected,
  enabled,
  pinnedThreadIds = NO_PINS,
  pubkey,
  relayUrl,
}: {
  capabilityKey: string | null;
  /** Fetch only while connected; loaded rows stay shown through a disconnect. */
  connected: boolean;
  enabled: boolean;
  pinnedThreadIds?: readonly string[];
  pubkey: string | undefined;
  relayUrl: string | undefined;
}): ChatListState | undefined {
  const queryClient = useQueryClient();
  const active = enabled && capabilityKey !== null && !!pubkey && !!relayUrl;
  const fetching = active && connected;
  const binding = React.useMemo(
    () => ({ pubkey: pubkey ?? "", relayUrl: relayUrl ?? "" }),
    [pubkey, relayUrl],
  );
  const queryKey = chatListQueryKey(
    relayUrl ?? "",
    pubkey ?? "",
    pinnedThreadIds,
  );
  const markUnavailable = (error: unknown) => {
    if (capabilityKey && isChatStateUnavailable(error)) {
      setChatStateCapability(capabilityKey, "off");
    }
  };
  const query = useQuery({
    enabled: fetching,
    queryKey,
    queryFn: async (): Promise<ChatListState> => {
      const stopRecording = recordChatListPatches(queryClient);
      try {
        const page = await getChatListPage(
          binding,
          null,
          pinnedThreadIds,
          CHAT_LIST_PAGE_SIZE,
        );
        const merged = mergeRefetchedFirstPage(
          queryClient.getQueryData<ChatListState>(queryKey),
          page,
          ++chatListGeneration,
        );
        return replayChatListPatches(merged, stopRecording());
      } catch (error) {
        stopRecording();
        markUnavailable(error);
        throw error;
      }
    },
    refetchOnWindowFocus: false,
    retry: (failureCount, error) =>
      !isChatStateUnavailable(error) && failureCount < 1,
    staleTime: Number.POSITIVE_INFINITY,
  });
  const data = active ? query.data : undefined;
  const next = data?.next ?? null;
  const wantsMore =
    fetching &&
    next !== null &&
    (data?.rows.length ?? 0) < CHAT_LIST_BACKGROUND_ROWS;
  const generation = data?.generation;
  const queryKeyText = queryKey.join("\u0000");
  // biome-ignore lint/correctness/useExhaustiveDependencies: keyed by the cursor; live row patches must not restart a page in flight
  React.useEffect(() => {
    if (!wantsMore || next === null) return;
    let cancelled = false;
    getChatListPage(binding, next, pinnedThreadIds, CHAT_LIST_PAGE_SIZE)
      .then((page) => {
        if (cancelled) return;
        queryClient.setQueryData<ChatListState>(queryKey, (current) =>
          // A newer first page or another page landed meanwhile.
          current !== undefined &&
          current.generation === generation &&
          current.next?.key === next.key &&
          current.next.at_us === next.at_us
            ? mergeChatListPage(current, page)
            : current,
        );
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        markUnavailable(error);
        // The next refetch (ping, reconnect) restarts paging.
        console.warn("chat list page failed");
      });
    return () => {
      cancelled = true;
    };
  }, [generation, next?.at_us, next?.key, wantsMore, queryKeyText]);

  return data;
}
