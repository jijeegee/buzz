import {
  BellOff,
  CheckCircle2,
  CircleCheck,
  CircleDot,
  Pin,
  PinOff,
  RotateCcw,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";

import type { ChatListRow } from "@/features/chats/chatListState";
import {
  threadRowMenuActions,
  type ThreadRowMenuAction,
} from "@/features/sidebar/lib/chatRowMenu";
import {
  ContextMenuIconSlot,
  deferMenuAction,
} from "@/features/sidebar/ui/sidebarMenuHelpers";
import { ContextMenuItem } from "@/shared/ui/context-menu";

/** What a thread row's menu does; the same actions as the thread header. */
export type ChatThreadRowActions = {
  pin: (rootId: string) => void;
  unpin: (rootId: string) => void;
  /** `at`: unix seconds of the newest message to count as read. */
  markRead: (rootId: string, at: number) => void;
  markUnread: (rootId: string) => void;
  unfollow: (rootId: string) => void;
  setClosed: (channelId: string, rootId: string, closed: boolean) => void;
};

const ITEMS: Record<ThreadRowMenuAction, { icon: LucideIcon; label: string }> =
  {
    pin: { icon: Pin, label: "Pin" },
    unpin: { icon: PinOff, label: "Unpin" },
    "mark-read": { icon: CheckCircle2, label: "Mark as read" },
    "mark-unread": { icon: CircleDot, label: "Mark as unread" },
    unfollow: { icon: BellOff, label: "Unfollow thread" },
    close: { icon: CircleCheck, label: "Close thread" },
    reopen: { icon: RotateCcw, label: "Reopen thread" },
  };

function runAction(
  action: ThreadRowMenuAction,
  actions: ChatThreadRowActions,
  row: ChatListRow,
  rootId: string,
) {
  switch (action) {
    case "pin":
      return actions.pin(rootId);
    case "unpin":
      return actions.unpin(rootId);
    case "mark-read":
      return actions.markRead(rootId, row.latest?.created_at ?? row.at);
    case "mark-unread":
      return actions.markUnread(rootId);
    case "unfollow":
      return actions.unfollow(rootId);
    case "close":
      return actions.setClosed(row.channel_id, rootId, true);
    case "reopen":
      return actions.setClosed(row.channel_id, rootId, false);
  }
}

export function ChatThreadRowMenuItems({
  actions,
  hasUnread,
  isPinned,
  row,
  rootId,
}: {
  actions: ChatThreadRowActions;
  hasUnread: boolean;
  isPinned: boolean;
  row: ChatListRow;
  rootId: string;
}) {
  return threadRowMenuActions({
    hasUnread,
    isClosed: row.closed,
    isPinned,
  }).map((action) => {
    const { icon: Icon, label } = ITEMS[action];
    return (
      <ContextMenuItem
        data-testid={`chat-thread-menu-${action}`}
        key={action}
        onSelect={() =>
          deferMenuAction(() => runAction(action, actions, row, rootId))
        }
      >
        <ContextMenuIconSlot>
          <Icon aria-hidden="true" className="h-4 w-4" />
        </ContextMenuIconSlot>
        <span>{label}</span>
      </ContextMenuItem>
    );
  });
}
