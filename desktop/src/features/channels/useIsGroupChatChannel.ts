import { useChannelsQuery } from "@/features/channels/hooks";

import { isGroupChat } from "./lib/groupChat";

/** Whether `channelId` is an ownerless chat in the cached room list. */
export function useIsGroupChatChannel(channelId: string | null): boolean {
  const channels = useChannelsQuery().data;
  if (!channelId) return false;
  return isGroupChat(channels?.find((channel) => channel.id === channelId));
}
