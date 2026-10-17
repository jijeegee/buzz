import type { ChannelType } from "@/shared/api/types";

/**
 * How a room row's menu lets you go: channels and group DMs are left, a
 * 1:1 DM is hidden (it comes back with the next message).
 * `otherParticipantCount` counts the DM's people other than me.
 */
export function roomExitAction(
  channelType: ChannelType,
  otherParticipantCount: number,
): "leave" | "hide" {
  return channelType === "dm" && otherParticipantCount <= 1 ? "hide" : "leave";
}

export type ThreadRowMenuAction =
  | "pin"
  | "unpin"
  | "mark-read"
  | "mark-unread"
  | "unfollow"
  | "close"
  | "reopen";

/**
 * A thread row's menu, in order: Pin/Unpin · Mark as read/unread ·
 * Unfollow · Close/Reopen.
 */
export function threadRowMenuActions({
  hasUnread,
  isClosed,
  isPinned,
}: {
  hasUnread: boolean;
  isClosed: boolean;
  isPinned: boolean;
}): ThreadRowMenuAction[] {
  return [
    isPinned ? "unpin" : "pin",
    hasUnread ? "mark-read" : "mark-unread",
    "unfollow",
    isClosed ? "reopen" : "close",
  ];
}
