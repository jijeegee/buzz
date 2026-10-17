import { CircleCheck, MessagesSquare, Pin } from "lucide-react";

import {
  chatThreadTitle,
  formatChatCount,
  type ChatListRow,
  type ChatRowUnread,
} from "@/features/chats/chatListState";
import { cn } from "@/shared/lib/cn";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuTrigger,
} from "@/shared/ui/context-menu";
import { SidebarMenuButton, SidebarMenuItem } from "@/shared/ui/sidebar";
import {
  ChatThreadRowMenuItems,
  type ChatThreadRowActions,
} from "./ChatThreadRowMenu";

/**
 * One followed (or pinned) thread in the server chats list:
 * `#channel › thread name`, unread and @ counts from the relay, closed
 * threads (pinned only) dimmed with a check mark.
 */
export function ChatThreadRow({
  actions,
  channelLabel,
  isDm,
  isMuted,
  isPinned,
  onOpenThread,
  row,
  unread,
}: {
  actions?: ChatThreadRowActions;
  channelLabel: string;
  isDm: boolean;
  isMuted: boolean;
  isPinned: boolean;
  onOpenThread: (channelId: string, rootId: string) => void;
  row: ChatListRow;
  unread: ChatRowUnread;
}) {
  const rootId = row.root_id ?? row.key;
  const title = chatThreadTitle(row);
  const place = isDm ? channelLabel : `#${channelLabel}`;
  const hasUnread = unread.count > 0 || unread.marked;
  const dimmed = row.closed || (isMuted && unread.mentions === 0);
  const item = (
    <SidebarMenuItem className="group/menu-item">
      <SidebarMenuButton
        className={cn(
          "data-[active=true]:font-normal group-hover/menu-item:bg-sidebar-accent group-hover/menu-item:text-sidebar-foreground",
          hasUnread && !dimmed && "font-bold text-sidebar-foreground",
        )}
        data-chat-thread-id={rootId}
        data-testid={`chat-thread-${rootId}`}
        onClick={() => onOpenThread(row.channel_id, rootId)}
        tooltip={`${place} › ${title}`}
        type="button"
      >
        <MessagesSquare
          aria-hidden="true"
          className="h-4 w-4 shrink-0 opacity-80"
        />
        {isPinned ? (
          <span className="shrink-0">
            <Pin aria-hidden="true" className="h-3 w-3" />
            <span className="sr-only">Pinned for you</span>
          </span>
        ) : null}
        <span
          className={cn(
            "flex min-w-0 flex-1 items-center gap-1",
            dimmed && "sidebar-muted-content opacity-50 dark:opacity-45",
          )}
          data-sidebar-row-label
        >
          <span className="max-w-[45%] shrink-0 truncate text-sidebar-foreground/60">
            {place}
          </span>
          <span
            aria-hidden="true"
            className="shrink-0 text-sidebar-foreground/40"
          >
            ›
          </span>
          <span className="sr-only">thread</span>
          <span className="min-w-0 truncate">{title}</span>
        </span>
        {row.closed ? (
          <span
            className="ml-auto shrink-0"
            data-testid={`chat-thread-closed-${rootId}`}
          >
            <CircleCheck
              aria-hidden="true"
              className="h-3.5 w-3.5 text-sidebar-foreground/50"
            />
            <span className="sr-only">Closed</span>
          </span>
        ) : null}
        {unread.mentions > 0 ? (
          <span
            className="ml-auto flex h-5 min-w-5 shrink-0 items-center justify-center rounded-full bg-sky-600 px-1 text-2xs font-semibold leading-none text-white"
            data-testid={`chat-thread-mentions-${rootId}`}
          >
            @
            <span className="sr-only">
              {" "}
              {formatChatCount(unread.mentions, unread.mentionsOverflow)} unread
              mention{unread.mentions === 1 ? "" : "s"}
            </span>
          </span>
        ) : null}
        {unread.count > 0 ? (
          <span
            className={cn(
              "flex h-5 min-w-5 shrink-0 items-center justify-center rounded-full px-1 text-2xs font-semibold leading-none tabular-nums",
              unread.mentions > 0 ? "" : "ml-auto",
              isMuted
                ? "bg-sidebar-foreground/20 text-sidebar-foreground"
                : "bg-primary text-primary-foreground",
            )}
            data-testid={`chat-thread-unread-${rootId}`}
          >
            {formatChatCount(unread.count, unread.overflow)}
            <span className="sr-only">
              {" "}
              unread message{unread.count === 1 ? "" : "s"}
            </span>
          </span>
        ) : unread.marked ? (
          <span
            className="ml-auto h-2 w-2 shrink-0 rounded-full bg-primary"
            data-testid={`chat-thread-unread-dot-${rootId}`}
          >
            <span className="sr-only">unread</span>
          </span>
        ) : null}
      </SidebarMenuButton>
    </SidebarMenuItem>
  );
  if (!actions) return item;
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>{item}</ContextMenuTrigger>
      <ContextMenuContent>
        <ChatThreadRowMenuItems
          actions={actions}
          hasUnread={hasUnread}
          isPinned={isPinned}
          row={row}
          rootId={rootId}
        />
      </ContextMenuContent>
    </ContextMenu>
  );
}
