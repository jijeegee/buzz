import type { Channel } from "@/shared/api/types";
import { sortChannelsForSidebar } from "./channelSortPreference";

/** DMs are already participant-scoped by the provider, unlike public channels. */
export function isJoinedChat(channel: Channel): boolean {
  return channel.channelType === "dm" || channel.isMember;
}

/** Personal pins partition the selected community's list; only messages sort it. */
export function sortChatList(
  channels: Channel[],
  pinnedIds?: ReadonlySet<string>,
): Channel[] {
  const recent = sortChannelsForSidebar(channels, "recent", (left, right) =>
    left.id < right.id ? -1 : left.id > right.id ? 1 : 0,
  );
  return [
    ...recent.filter((channel) => pinnedIds?.has(channel.id)),
    ...recent.filter((channel) => !pinnedIds?.has(channel.id)),
  ];
}
