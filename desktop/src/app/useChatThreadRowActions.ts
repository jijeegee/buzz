import { useQueryClient } from "@tanstack/react-query";
import * as React from "react";

import { CHAT_LIST_QUERY_KEY } from "@/features/chats/chatListState";
import { useThreadCloseToggle } from "@/features/messages/useThreadCloseToggle";
import type { ServerChats } from "@/features/sidebar/ui/ChatList";
import type { ChatThreadRowActions } from "@/features/sidebar/ui/ChatThreadRowMenu";

/**
 * The server chats list with its thread rows' menu attached. Thread rows
 * exist only while the relay keeps chat state, so every action here may use
 * it; each one is the same action the thread header or message menu runs.
 */
export function useChatThreadRowActions({
  clearThreadUnreadGuard,
  markThreadRead,
  markThreadUnread,
  pinThread,
  serverChats,
  unfollowThread,
  unpinThread,
}: {
  clearThreadUnreadGuard: (rootId: string) => void;
  markThreadRead: (rootId: string, timestamp: number) => void;
  markThreadUnread: (rootId: string) => boolean;
  pinThread: (rootId: string) => void;
  serverChats: ServerChats | undefined;
  unfollowThread: (rootId: string) => void;
  unpinThread: (rootId: string) => void;
}): ServerChats | undefined {
  const queryClient = useQueryClient();
  const { setThreadClosed } = useThreadCloseToggle();
  const actions = React.useMemo<ChatThreadRowActions>(
    () => ({
      pin: pinThread,
      unpin: unpinThread,
      markRead: (rootId, at) => {
        clearThreadUnreadGuard(rootId);
        markThreadRead(rootId, at);
      },
      markUnread: (rootId) => void markThreadUnread(rootId),
      unfollow: unfollowThread,
      setClosed: (channelId, rootId, closed) =>
        setThreadClosed({
          channelId,
          rootId,
          closed,
          // The relay owns the row's closed state; ask for the list again.
          onSent: () =>
            void queryClient.invalidateQueries({
              queryKey: CHAT_LIST_QUERY_KEY,
            }),
        }),
    }),
    [
      clearThreadUnreadGuard,
      markThreadRead,
      markThreadUnread,
      pinThread,
      queryClient,
      setThreadClosed,
      unfollowThread,
      unpinThread,
    ],
  );
  return React.useMemo(
    () =>
      serverChats ? { ...serverChats, threadActions: actions } : undefined,
    [actions, serverChats],
  );
}
