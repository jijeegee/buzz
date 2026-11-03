import * as React from "react";
import { type QueryClient, useQueryClient } from "@tanstack/react-query";

import {
  CHAT_LIST_QUERY_KEY,
  type ChatListState,
  chatThreadTitle,
} from "@/features/chats/chatListState";
import type { ThreadRootOpen } from "@/features/messages/lib/independentThreadPanel";
import { useThreadNameLabel } from "@/features/messages/lib/useThreadName";
import { useThreadRoot } from "@/features/messages/useThreadRoot";
import type { Channel, RelayEvent } from "@/shared/api/types";
import { useT } from "@/shared/i18n";

/** The thread's title as its Chats row shows it, when one is cached. */
export function chatRowThreadTitle(
  queryClient: QueryClient,
  rootId: string,
): string | null {
  for (const [, state] of queryClient.getQueriesData<ChatListState>({
    queryKey: CHAT_LIST_QUERY_KEY,
  })) {
    const row = state?.rows?.find(
      (candidate) =>
        candidate.type === "thread" &&
        (candidate.root_id ?? candidate.key) === rootId,
    );
    if (row) return chatThreadTitle(row);
  }
  return null;
}

/**
 * The channel screen's thread panel opens on the root id alone
 * (docs/message-sync.md#opening-a-thread-from-its-root-id): resolves the root
 * in the background and names the placeholder top row. Off for forum rooms
 * (they keep the channel route) and huddle transcripts.
 */
export function useThreadRootOpen({
  activeChannel,
  isHuddleTranscript,
  loadedEvents,
  rootId,
}: {
  activeChannel: Channel | null;
  isHuddleTranscript: boolean;
  loadedEvents: readonly RelayEvent[];
  rootId: string | null;
}): ThreadRootOpen | null {
  const t = useT();
  const queryClient = useQueryClient();
  const channelId = activeChannel?.id ?? null;
  const enabled =
    channelId !== null &&
    rootId !== null &&
    !isHuddleTranscript &&
    activeChannel?.channelType !== "forum";
  const root = useThreadRoot({ channelId, enabled, loadedEvents, rootId });
  const threadName = useThreadNameLabel(
    enabled ? channelId : null,
    enabled ? rootId : null,
  ).trim();
  const placeholderTitle =
    threadName ||
    (enabled && rootId ? chatRowThreadTitle(queryClient, rootId) : null) ||
    t("chats.threadTitle.untitled");
  return React.useMemo(
    () =>
      enabled && channelId ? { channelId, root, placeholderTitle } : null,
    [channelId, enabled, placeholderTitle, root],
  );
}
