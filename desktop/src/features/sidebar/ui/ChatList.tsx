import { useMemo, type ComponentProps } from "react";
import { isJoinedChat, sortChatList } from "@/features/sidebar/lib/chatList";
import { SidebarSection } from "./SidebarSection";

/** The selected community's joined channels and visible DMs in one chat list. */
export function ChatList(
  props: Omit<ComponentProps<typeof SidebarSection>, "title" | "testId">,
) {
  const items = useMemo(
    () =>
      sortChatList(
        props.items.filter(
          (channel) =>
            channel.channelType !== "forum" &&
            isJoinedChat(channel) &&
            !channel.archivedAt,
        ),
        props.starredChannelIds,
      ),
    [props.items, props.starredChannelIds],
  );
  return (
    <SidebarSection {...props} items={items} title="Chats" testId="chat-list" />
  );
}
