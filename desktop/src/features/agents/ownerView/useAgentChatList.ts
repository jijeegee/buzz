import { useQuery } from "@tanstack/react-query";
import * as React from "react";

import { useCommunities } from "@/features/communities/useCommunities";
import type { ChatListRow } from "@/features/chats/chatListState";
import { useIdentityQuery } from "@/shared/api/hooks";
import { getAgentChatListPage } from "@/shared/api/chatState";
import { useRelayConnection } from "@/shared/api/useRelayConnection";

import {
  AGENT_CHAT_LIST_PAGE_SIZE,
  agentChatListQueryKey,
  loadAgentChatList,
  withSeenDots,
} from "./agentChatList";
import { ensureOwnerViewBaseline, useOwnerViewSeen } from "./ownerViewSeen";

/** The relay sends no live updates for these rows; poll while shown. */
const AGENT_CHAT_LIST_REFRESH_MS = 30_000;

/** The signed-in owner and relay the owner view reads as. */
export function useOwnerViewBinding() {
  const ownerPubkey = useIdentityQuery().data?.pubkey;
  const relayUrl = useCommunities().activeCommunity?.relayUrl;
  return React.useMemo(
    () =>
      ownerPubkey && relayUrl
        ? { ownerPubkey: ownerPubkey.toLowerCase(), relayUrl }
        : null,
    [ownerPubkey, relayUrl],
  );
}

/**
 * One agent's chats without its owner, newest first, with dots on what
 * moved since the owner last looked on this device.
 */
export function useAgentChatList(agentPubkey: string | null) {
  const binding = useOwnerViewBinding();
  const connected = useRelayConnection() === "connected";
  const agent = agentPubkey?.toLowerCase() ?? "";
  const query = useQuery({
    enabled: binding !== null && agent.length === 64 && connected,
    queryKey: agentChatListQueryKey(
      binding?.relayUrl ?? "",
      binding?.ownerPubkey ?? "",
      agent,
    ),
    queryFn: async (): Promise<ChatListRow[]> => {
      if (!binding) return [];
      const rows = await loadAgentChatList((before) =>
        getAgentChatListPage(
          { pubkey: binding.ownerPubkey, relayUrl: binding.relayUrl },
          agent,
          before,
          AGENT_CHAT_LIST_PAGE_SIZE,
        ),
      );
      ensureOwnerViewBaseline(binding.ownerPubkey, agent);
      return rows;
    },
    refetchInterval: AGENT_CHAT_LIST_REFRESH_MS,
    refetchOnWindowFocus: true,
    retry: 1,
    staleTime: 10_000,
  });
  const seenAt = useOwnerViewSeen(binding?.ownerPubkey, agent);
  const rows = React.useMemo(
    () => (query.data ? withSeenDots(query.data, agent, seenAt) : undefined),
    [agent, query.data, seenAt],
  );
  return {
    error: query.error,
    isPending: query.isPending,
    ownerPubkey: binding?.ownerPubkey,
    rows,
  };
}
