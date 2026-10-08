import * as React from "react";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import { useChannelsQuery } from "@/features/channels/hooks";
import { HomeScreen } from "@/features/home/ui/HomeScreen";
import { useIdentityQuery } from "@/shared/api/hooks";

/**
 * The inbox list pulled out beside a channel's chat screen. Rows enter their
 * chat room in place, so Inbox and Chats share one screen; the sidebar's Inbox
 * button shows and hides this panel.
 */
export function InboxPanel() {
  const { goChannel } = useAppNavigation();
  const channelsQuery = useChannelsQuery();
  const identityQuery = useIdentityQuery();
  const channels = channelsQuery.data;
  const availableChannelIds = React.useMemo(
    () => new Set((channels ?? []).map((channel) => channel.id)),
    [channels],
  );

  return (
    <HomeScreen
      availableChannelIds={availableChannelIds}
      currentPubkey={identityQuery.data?.pubkey}
      onOpenContext={(channelId, messageId, threadRootId) => {
        // A thread row opens its thread with the navigation itself, so moving
        // between threads swaps only the thread panel.
        void goChannel(channelId, {
          messageId: messageId ?? undefined,
          thread: messageId ? (threadRootId ?? undefined) : undefined,
          threadRootId,
        });
      }}
      variant="panel"
    />
  );
}
