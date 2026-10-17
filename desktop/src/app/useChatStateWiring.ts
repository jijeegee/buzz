import { useQueryClient } from "@tanstack/react-query";
import * as React from "react";

import type { ReadStateServerMirror } from "@/features/channels/readState/readStateManager";
import { CHAT_LIST_QUERY_KEY } from "@/features/chats/chatListState";
import {
  getChatStateCapability,
  setChatStateCapability,
} from "@/features/chats/chatStateCapability";
import { ChatStateWriter } from "@/features/chats/chatStateWrites";
import type { ServerChats } from "@/features/sidebar/ui/ChatList";
import { useChatStateCapability } from "@/features/chats/useChatStateCapability";
import { useChatStatePing } from "@/features/chats/useChatStatePing";
import { useServerChatList } from "@/features/chats/useServerChatList";
import {
  chatMarkRead,
  chatThreadFollow,
  type ChatFollowState,
} from "@/shared/api/chatState";
import { useRelayConnection } from "@/shared/api/useRelayConnection";

/**
 * Chats S4a: the relay's chat state next to today's device-local state.
 * While the relay keeps chat state ("on"), the sidebar Chats list renders the
 * server rows and this device's reads and follow choices are mirrored to it.
 * Otherwise (off, unknown) nothing changes: `serverChats` is undefined and
 * no writes are sent. Local read markers (30078), thread follows and thread
 * mutes keep working unchanged.
 */
export function useChatStateWiring({
  getOwnReadAt,
  openThread,
  pubkey,
  readStateVersion,
  relayUrl,
  setReadStateServerMirror,
}: {
  getOwnReadAt: (contextId: string) => number | null;
  openThread: (channelId: string, rootId: string) => void;
  pubkey: string | undefined;
  readStateVersion: number;
  relayUrl: string | undefined;
  setReadStateServerMirror: (mirror: ReadStateServerMirror | null) => void;
}): {
  mirrorFollow: (rootId: string, state: ChatFollowState) => void;
  serverChats: ServerChats | undefined;
} {
  const queryClient = useQueryClient();
  const connected = useRelayConnection() === "connected";
  const { capability, capabilityKey, reprobe } = useChatStateCapability({
    connected,
    pubkey,
    relayUrl,
  });
  const writerRef = React.useRef<ChatStateWriter | null>(null);

  React.useEffect(() => {
    if (capabilityKey === null) return;
    const writer = new ChatStateWriter({
      markRead: chatMarkRead,
      follow: chatThreadFollow,
      capability: () => getChatStateCapability(capabilityKey),
      onUnavailable: () => setChatStateCapability(capabilityKey, "off"),
    });
    writerRef.current = writer;
    setReadStateServerMirror((contextId, at) =>
      writer.queueRead(contextId, at),
    );
    const flushWhenHidden = () => {
      if (document.visibilityState === "hidden") void writer.flush();
    };
    const flushNow = () => void writer.flush();
    document.addEventListener("visibilitychange", flushWhenHidden);
    window.addEventListener("pagehide", flushNow);
    return () => {
      document.removeEventListener("visibilitychange", flushWhenHidden);
      window.removeEventListener("pagehide", flushNow);
      setReadStateServerMirror(null);
      void writer.flush().finally(() => writer.dispose());
      if (writerRef.current === writer) writerRef.current = null;
    };
  }, [capabilityKey, setReadStateServerMirror]);

  React.useEffect(() => {
    void capability;
    writerRef.current?.capabilityChanged();
  }, [capability]);

  const on = capability === "on";
  useChatStatePing(pubkey, connected && capabilityKey !== null, () => {
    if (getChatStateCapability(capabilityKey) === "on") {
      void queryClient.invalidateQueries({ queryKey: CHAT_LIST_QUERY_KEY });
    } else {
      reprobe();
    }
  });
  const chatList = useServerChatList({
    capabilityKey,
    connected,
    enabled: on,
    pubkey,
    relayUrl,
  });

  const mirrorFollow = React.useCallback(
    (rootId: string, state: ChatFollowState) =>
      writerRef.current?.setFollow(rootId, state),
    [],
  );
  const rows = on ? chatList?.rows : undefined;
  const serverChats = React.useMemo<ServerChats | undefined>(
    () =>
      rows
        ? {
            getReadAt: getOwnReadAt,
            onOpenThread: openThread,
            readStateVersion,
            rows,
          }
        : undefined,
    [getOwnReadAt, openThread, readStateVersion, rows],
  );
  return { mirrorFollow, serverChats };
}
