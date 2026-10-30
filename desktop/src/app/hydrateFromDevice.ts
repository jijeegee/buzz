import type { QueryClient } from "@tanstack/react-query";

import { prefetchChatRowsSnapshot } from "@/features/chats/chatRowsSnapshot";
import { hydrateChannelHeads } from "@/features/messages/lib/channelHeadCache";
import { hydrateUserLabels } from "@/features/profile/lib/userLabelStorage";

/**
 * What a community's new query cache reads from the device before any relay
 * answer (docs/message-sync.md#launch-order): stored channel heads, the
 * Chats rows and other people's labels kept from the last run. All local
 * reads, started together; none blocks the screen.
 */
export function hydrateFromDevice(
  queryClient: QueryClient,
  scope: { pubkey: string; relayUrl: string },
): Promise<void> {
  prefetchChatRowsSnapshot(queryClient, scope);
  void hydrateUserLabels(scope);
  return hydrateChannelHeads(queryClient, scope);
}
