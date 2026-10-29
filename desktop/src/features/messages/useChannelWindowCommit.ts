import * as React from "react";
import type { QueryClient } from "@tanstack/react-query";

import {
  channelWindowCommit,
  subscribeChannelWindowCommits,
} from "@/features/messages/lib/projectChannelWindow";

/**
 * The room's last fetched window, re-rendering when a new one is committed:
 * a relay page equal to the cached one changes no query data, yet it makes
 * the window current.
 */
export function useChannelWindowCommit(
  queryClient: QueryClient,
  channelId: string | null,
) {
  const at = React.useSyncExternalStore(
    subscribeChannelWindowCommits,
    () => channelWindowCommit(queryClient, channelId)?.at ?? null,
  );
  return at === null ? null : channelWindowCommit(queryClient, channelId);
}
