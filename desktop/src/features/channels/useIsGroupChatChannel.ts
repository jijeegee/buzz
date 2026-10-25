import * as React from "react";

import { useChannelsQuery } from "@/features/channels/hooks";
import type { Channel } from "@/shared/api/types";

import { isGroupChat } from "./lib/groupChat";

/** Whether `channelId` is an ownerless chat in the cached room list. */
export function useIsGroupChatChannel(channelId: string | null): boolean {
  // A boolean slice: timeline rows must not re-render on other rooms' events.
  const select = React.useCallback(
    (channels: Channel[]) =>
      channelId !== null &&
      isGroupChat(channels.find((channel) => channel.id === channelId)),
    [channelId],
  );
  return useChannelsQuery({ select }).data ?? false;
}
