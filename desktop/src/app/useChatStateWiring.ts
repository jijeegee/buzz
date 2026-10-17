import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import * as React from "react";

import type { ChatStateSyncProps } from "@/app/ChatStateSync";
import type { ForcedUnreadSource } from "@/features/channels/forcedUnreadStore";
import type { ReadStateServerMirror } from "@/features/channels/readState/readStateManager";
import {
  applyFollowResponseToChatList,
  applyMarkReadResponseToChatList,
  CHAT_LIST_QUERY_KEY,
  type ChatListRow,
} from "@/features/chats/chatListState";
import { patchChatLists } from "@/features/chats/chatListSync";
import {
  chatStateCapabilityKey,
  getChatStateCapability,
  setChatStateCapability,
  subscribeChatStateCapability,
} from "@/features/chats/chatStateCapability";
import {
  ChatStateImporter,
  type ChatImportSources,
} from "@/features/chats/chatStateImport";
import {
  ChatStateWriter,
  type ChatStateWritten,
} from "@/features/chats/chatStateWrites";
import { mutedStore } from "@/features/channels/unreadMembership";
import { readThreadFollowRootIds } from "@/features/messages/lib/useThreadFollows";
import type { ServerChats } from "@/features/sidebar/ui/ChatList";
import {
  chatImport,
  chatMarkRead,
  chatThreadFollow,
  type ChatFollowState,
} from "@/shared/api/chatState";
import type { Channel } from "@/shared/api/types";

/** This device's stored reads, follows and mutes for `pubkey`, read now. */
function collectImportSources(
  pubkey: string,
  getReadContexts: () => Record<string, number>,
): ChatImportSources {
  const lower = pubkey.toLowerCase();
  return {
    contexts: getReadContexts(),
    followedRootIds: readThreadFollowRootIds(pubkey),
    // Mutes are written under the lowercased pubkey.
    mutedRootIds: new Set([
      ...mutedStore.read(pubkey),
      ...mutedStore.read(lower),
    ]),
  };
}

/** A room this device knows the user has not joined (DMs are always joined). */
function isKnownNonMemberRoom(channels: readonly Channel[], id: string) {
  const channel = channels.find((candidate) => candidate.id === id);
  return !!channel && channel.channelType !== "dm" && !channel.isMember;
}

/** Patch the shown list from this device's own write instead of refetching. */
function applyOwnWrite(queryClient: QueryClient, written: ChatStateWritten) {
  if (written.type === "mark_read") {
    patchChatLists(queryClient, (state) =>
      applyMarkReadResponseToChatList(state, written.response),
    );
    return;
  }
  let refetch = false;
  patchChatLists(queryClient, (state, replay) => {
    const patch = applyFollowResponseToChatList(state, written.response);
    if (!replay) refetch ||= patch.refetch;
    return patch.state;
  });
  if (refetch) {
    void queryClient.invalidateQueries({ queryKey: CHAT_LIST_QUERY_KEY });
  }
}

/**
 * Chats S4a: the relay's chat state next to today's device-local state.
 * While the relay keeps chat state ("on"), the sidebar Chats list renders the
 * server rows and this device's reads, unread marks and follow choices are
 * mirrored to it, bound to the account (relay + pubkey) they were made under.
 * Otherwise (off, unknown) nothing changes: `serverChats` is undefined and
 * no writes are sent. Local read markers (30078), thread follows and thread
 * mutes keep working unchanged.
 *
 * This hook re-renders only when the capability or the server rows change;
 * the connection-driven parts (probe, ping, list query) live in
 * `<ChatStateSync {...syncProps} />`.
 */
export function useChatStateWiring({
  channels,
  forcedUnreadChannelIds,
  getOwnReadAt,
  getReadContexts,
  isReadStateInitialized,
  markChannelUnread,
  mutedRootIds,
  openThread,
  pubkey,
  readStateVersion,
  relayUrl,
  setReadStateServerMirror,
}: {
  channels: readonly Channel[];
  forcedUnreadChannelIds: ReadonlySet<string>;
  getOwnReadAt: (contextId: string) => number | null;
  /** Local channel and thread read markers, for the first-run upload. */
  getReadContexts: () => Record<string, number>;
  isReadStateInitialized: () => boolean;
  markChannelUnread: (channelId: string, source?: ForcedUnreadSource) => void;
  mutedRootIds: ReadonlySet<string>;
  openThread: (channelId: string, rootId: string) => void;
  pubkey: string | undefined;
  readStateVersion: number;
  relayUrl: string | undefined;
  setReadStateServerMirror: (mirror: ReadStateServerMirror | null) => void;
}): {
  markChannelUnread: (channelId: string, source?: ForcedUnreadSource) => void;
  mirrorFollow: (rootId: string, state: ChatFollowState) => void;
  serverChats: ServerChats | undefined;
  syncProps: ChatStateSyncProps;
} {
  const queryClient = useQueryClient();
  const capabilityKey = chatStateCapabilityKey(relayUrl, pubkey);
  const capability = React.useSyncExternalStore(
    subscribeChatStateCapability,
    () => getChatStateCapability(capabilityKey),
  );
  const writerRef = React.useRef<ChatStateWriter | null>(null);
  const channelsRef = React.useRef(channels);
  React.useEffect(() => {
    channelsRef.current = channels;
  }, [channels]);
  const [rows, setRows] = React.useState<readonly ChatListRow[] | undefined>(
    undefined,
  );

  React.useEffect(() => {
    if (capabilityKey === null || !pubkey || !relayUrl) return;
    // Every write carries the account it was queued under; the backend
    // refuses it once that is no longer the active one.
    const binding = { pubkey, relayUrl };
    const writer = new ChatStateWriter({
      markRead: (items, unreadMark) => chatMarkRead(binding, items, unreadMark),
      follow: (root, state) => chatThreadFollow(binding, root, state),
      capability: () => getChatStateCapability(capabilityKey),
      onUnavailable: () => setChatStateCapability(capabilityKey, "off"),
      onWritten: (written) => applyOwnWrite(queryClient, written),
      isKnownNonMember: (item) =>
        item.kind === "channel" &&
        isKnownNonMemberRoom(channelsRef.current, item.id),
    });
    writerRef.current = writer;
    setReadStateServerMirror((contextId, at) =>
      writer.queueRead(contextId, at),
    );
    // Hiding or closing the app (same account) sends what is queued.
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
      // Account, community or relay switch, or teardown: what is still
      // queued belongs to the old account and is dropped, never sent later.
      writer.dispose();
      if (writerRef.current === writer) writerRef.current = null;
    };
  }, [capabilityKey, pubkey, queryClient, relayUrl, setReadStateServerMirror]);

  React.useEffect(() => {
    void capability;
    writerRef.current?.capabilityChanged();
  }, [capability]);

  // S4b: upload this device's reads and follows once per account, after the
  // local markers are complete (readStateVersion bumps when they are).
  const [importer] = React.useState(
    () =>
      new ChatStateImporter({
        send: (binding, chunk) =>
          chatImport(binding, chunk.reads, chunk.follows),
        onUnavailable: (key) => setChatStateCapability(key, "off"),
        onImported: () =>
          void queryClient.invalidateQueries({ queryKey: CHAT_LIST_QUERY_KEY }),
      }),
  );
  React.useEffect(() => {
    void readStateVersion;
    void importer.update({
      key: capabilityKey,
      binding: pubkey && relayUrl ? { pubkey, relayUrl } : null,
      capability,
      readStateInitialized: isReadStateInitialized(),
      collect: () => collectImportSources(pubkey ?? "", getReadContexts),
    });
  }, [
    capability,
    capabilityKey,
    getReadContexts,
    importer,
    isReadStateInitialized,
    pubkey,
    readStateVersion,
    relayUrl,
  ]);
  React.useEffect(() => () => importer.stop(), [importer]);

  const mirrorFollow = React.useCallback(
    (rootId: string, state: ChatFollowState) =>
      writerRef.current?.setFollow(rootId, state),
    [],
  );
  const markChannelUnreadMirrored = React.useCallback(
    (channelId: string, source: ForcedUnreadSource = "manual") => {
      markChannelUnread(channelId, source);
      // Only the user's own "Mark as unread"; Inbox marks stay local.
      if (source === "manual") writerRef.current?.queueUnread(channelId);
    },
    [markChannelUnread],
  );

  const on = capability === "on";
  const shownRows = on ? rows : undefined;
  const serverChats = React.useMemo<ServerChats | undefined>(
    () =>
      shownRows
        ? {
            forcedUnreadChannelIds,
            getReadAt: getOwnReadAt,
            mutedRootIds,
            onOpenThread: openThread,
            readStateVersion,
            rows: shownRows,
          }
        : undefined,
    [
      forcedUnreadChannelIds,
      getOwnReadAt,
      mutedRootIds,
      openThread,
      readStateVersion,
      shownRows,
    ],
  );
  const syncProps = React.useMemo<ChatStateSyncProps>(
    () => ({ capabilityKey, onRows: setRows, pubkey, relayUrl, writerRef }),
    [capabilityKey, pubkey, relayUrl],
  );
  return {
    markChannelUnread: markChannelUnreadMirrored,
    mirrorFollow,
    serverChats,
    syncProps,
  };
}
