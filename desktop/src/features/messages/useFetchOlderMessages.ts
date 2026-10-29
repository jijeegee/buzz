import { useCallback, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";

import { channelWindowKey } from "@/features/messages/lib/messageQueryKeys";
import {
  channelWindowHasMore,
  channelWindowHistoryExhausted,
  emptyChannelWindowStore,
  type ChannelWindowStore,
} from "@/features/messages/lib/channelWindowStore";
import { pageOlderMessagesUntilRowFloor } from "@/features/messages/lib/pageOlderMessages";
import type { Channel } from "@/shared/api/types";

export function useFetchOlderMessages(channel: Channel | null) {
  const queryClient = useQueryClient();
  const channelId = channel?.id ?? null;
  const [isFetchingOlder, setIsFetchingOlder] = useState(false);
  const isFetchingOlderRef = useRef(false);

  // Whether older history remains, derived reactively from the authoritative
  // window store rather than a private latch. A latch only reset on channelId
  // change went stale on reconnect: `refreshNewestWindow` replaces the newest
  // window with fresh `hasMore:true` rows, but the latch — flipped false when
  // the pre-reconnect window exhausted — kept the scroll observer uninstalled,
  // freezing paging at page one. Reading the store's tail `hasMore` self-heals:
  // the observer re-arms the moment the refreshed window reports more history.
  const windowKey = channelWindowKey(channelId ?? "none");
  const { data: hasOlderMessages = false } = useQuery({
    enabled: channelId !== null,
    queryKey: windowKey,
    select: channelWindowHasMore,
    // Passive subscription: the window store is written by the messages query
    // and the live subscription via setQueryData; this observer only reads.
    queryFn: () =>
      queryClient.getQueryData<ChannelWindowStore>(windowKey) ??
      emptyChannelWindowStore(),
  });

  // Distinct from `!hasOlderMessages`: an empty/unloaded window also reports
  // "no more", but exhaustion requires a RESOLVED tail page proving the
  // channel's beginning. Consumers gating UI on the history boundary (the
  // oldest day divider) must use this, not the paging signal.
  const { data: historyExhausted = false } = useQuery({
    enabled: channelId !== null,
    queryKey: windowKey,
    select: channelWindowHistoryExhausted,
    queryFn: () =>
      queryClient.getQueryData<ChannelWindowStore>(windowKey) ??
      emptyChannelWindowStore(),
  });

  /** Whether a page was appended; false when skipped or it failed. */
  const fetchOlder = useCallback(
    async (pageSize?: number) => {
      if (!channelId || isFetchingOlderRef.current) {
        return false;
      }
      const store =
        queryClient.getQueryData<ChannelWindowStore>(
          channelWindowKey(channelId),
        ) ?? emptyChannelWindowStore();
      if (!channelWindowHasMore(store)) {
        return false;
      }

      isFetchingOlderRef.current = true;
      setIsFetchingOlder(true);
      try {
        await pageOlderMessagesUntilRowFloor(
          queryClient,
          channelId,
          () => channelId === channel?.id,
          // Callers wired as event handlers pass an event here.
          typeof pageSize === "number" ? pageSize : undefined,
        );
        return true;
      } catch (error) {
        console.error("Failed to fetch older messages", channelId, error);
        return false;
      } finally {
        isFetchingOlderRef.current = false;
        setIsFetchingOlder(false);
      }
    },
    [channel?.id, channelId, queryClient],
  );

  return { fetchOlder, isFetchingOlder, hasOlderMessages, historyExhausted };
}
