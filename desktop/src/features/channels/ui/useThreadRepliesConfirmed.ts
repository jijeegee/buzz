import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";

import {
  isThreadConfirmed,
  subscribeThreadConfirmations,
} from "@/features/messages/lib/storeThreadHead";

/**
 * Whether the open thread's replies come from the relay, not only from a
 * stored paint still waiting for it (docs/message-sync.md#painting-from-the-store).
 */
export function useThreadRepliesConfirmed(
  channelId: string | null | undefined,
  rootId: string | null | undefined,
): boolean {
  const queryClient = useQueryClient();
  return React.useSyncExternalStore(subscribeThreadConfirmations, () =>
    channelId && rootId
      ? isThreadConfirmed(queryClient, channelId, rootId)
      : true,
  );
}
