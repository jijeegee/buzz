import { type QueryClient, useQueryClient } from "@tanstack/react-query";
import * as React from "react";

import { channelsQueryKey } from "@/features/channels/hooks";
import {
  channelReferenceQueryKey,
  isChannelReferenceOpenable,
} from "@/features/channels/openChannelDirectory";
import { useChatRowChannel } from "@/features/chats/chatRowChannel";
import { storeScope } from "@/features/messages/lib/paintCoverage";
import type { Channel } from "@/shared/api/types";

/** Rooms whose missing list entry was asked for again, per query cache. */
const listRefreshed = new WeakMap<QueryClient, Set<string>>();

/**
 * A room the channel list does not hold, opened from its Chats row
 * (chatRowChannel) instead of a loading screen or the empty pane. Null when
 * no row names it, when `enabled` is false, or once the relay answered that
 * the room does not exist or cannot be opened (the route's not-found
 * handling stays). The channel only stands in for display: no members, no
 * sends (isChatRowStandIn). When the
 * loaded list lacks the room, the list is asked again once, so the full
 * channel replaces the row's soon rather than at the next poll.
 */
export function useChannelFromRow({
  channelId,
  enabled,
  listLoaded,
}: {
  channelId: string;
  enabled: boolean;
  listLoaded: boolean;
}): Channel | null {
  const queryClient = useQueryClient();
  const channel = useChatRowChannel(
    channelId,
    enabled,
    storeScope(queryClient) ?? null,
  );
  const reference = queryClient.getQueryState<Channel | null>(
    channelReferenceQueryKey(channelId),
  );
  const notFound =
    reference?.status === "success" &&
    !isChannelReferenceOpenable(reference.data ?? undefined);
  const shown = notFound ? null : channel;
  React.useEffect(() => {
    if (!shown || !listLoaded) return;
    let asked = listRefreshed.get(queryClient);
    if (!asked) {
      asked = new Set();
      listRefreshed.set(queryClient, asked);
    }
    if (asked.has(channelId)) return;
    asked.add(channelId);
    void queryClient.invalidateQueries({
      queryKey: channelsQueryKey,
      exact: true,
    });
  }, [channelId, listLoaded, queryClient, shown]);
  return shown;
}
