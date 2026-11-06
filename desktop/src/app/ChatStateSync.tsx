import { useQueryClient } from "@tanstack/react-query";
import * as React from "react";

import {
  CHAT_LIST_QUERY_KEY,
  type ChatListRow,
} from "@/features/chats/chatListState";
import type { ChatStateWriter } from "@/features/chats/chatStateWrites";
import { useChatStateCapability } from "@/features/chats/useChatStateCapability";
import { useChatStatePing } from "@/features/chats/useChatStatePing";
import { useServerChatList } from "@/features/chats/useServerChatList";
import { AGENT_PAIRINGS_KEY } from "@/features/agents/externalConnect/useExternalAgents";
import { groupInvitationsQueryKey } from "@/features/groups/hooks";
import { AUTH_DEVICES_QUERY_KEY } from "@/shared/api/useAuthDeviceNames";
import { useRelayConnection } from "@/shared/api/useRelayConnection";

export type ChatStateSyncProps = {
  capabilityKey: string | null;
  /** The server rows while the capability is on, else undefined. */
  onRows: (rows: readonly ChatListRow[] | undefined) => void;
  /** Threads pinned on any of my devices; the relay lists them too. */
  pinnedThreadIds: readonly string[];
  pubkey: string | undefined;
  relayUrl: string | undefined;
  writerRef: React.RefObject<ChatStateWriter | null>;
};

/**
 * The connection-driven half of the chat-state wiring, kept out of AppShell
 * so connection changes re-render only this: the capability probe, the
 * chat-state ping (subscribed only while the capability is on) and the
 * server list query. Renders nothing.
 */
export function ChatStateSync({
  capabilityKey,
  onRows,
  pinnedThreadIds,
  pubkey,
  relayUrl,
  writerRef,
}: ChatStateSyncProps) {
  const queryClient = useQueryClient();
  const connected = useRelayConnection() === "connected";
  const { capability } = useChatStateCapability({
    connected,
    pubkey,
    relayUrl,
  });
  const on = capability === "on";
  const refreshInvitations = React.useEffectEvent(() => {
    if (!pubkey) return;
    void queryClient.invalidateQueries({
      queryKey: groupInvitationsQueryKey(pubkey.toLowerCase()),
    });
  });
  // A ping sent while offline is gone.
  React.useEffect(() => {
    if (connected && on) refreshInvitations();
  }, [connected, on]);
  useChatStatePing(
    pubkey,
    connected && on,
    () => {
      // This device's own write already patched the list from its response.
      if (writerRef.current?.explainsPing()) return;
      void queryClient.invalidateQueries({ queryKey: CHAT_LIST_QUERY_KEY });
    },
    refreshInvitations,
    () => {
      // Another of my devices connected, disconnected or renamed something.
      // Cancel first: a refetch would only join a retry sitting out its wait.
      void queryClient
        .cancelQueries({ queryKey: AUTH_DEVICES_QUERY_KEY })
        .then(() =>
          queryClient.invalidateQueries({ queryKey: AUTH_DEVICES_QUERY_KEY }),
        );
      void queryClient.invalidateQueries({ queryKey: AGENT_PAIRINGS_KEY });
    },
  );
  const chatList = useServerChatList({
    capabilityKey,
    connected,
    enabled: on,
    pinnedThreadIds,
    pubkey,
    relayUrl,
  });
  const rows = on ? chatList?.rows : undefined;
  React.useEffect(() => {
    onRows(rows);
  }, [onRows, rows]);
  React.useEffect(() => () => onRows(undefined), [onRows]);
  return null;
}
