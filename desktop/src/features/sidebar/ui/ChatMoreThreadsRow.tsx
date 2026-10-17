import { formatChatCount } from "@/features/chats/chatListState";
import { cn } from "@/shared/lib/cn";
import { SidebarMenuButton, SidebarMenuItem } from "@/shared/ui/sidebar";

/**
 * By channel: "+ N more threads" under a channel's first threads, with the
 * hidden threads' summed unread and @, so hidden unread is never invisible.
 * A hidden thread marked unread with nothing to count shows a dot.
 * Expanded, the same line reads "Show less".
 */
export function ChatMoreThreadsRow({
  channelId,
  count,
  expanded,
  isMuted,
  marked,
  mentions,
  onToggle,
  unread,
  unreadOverflow,
}: {
  channelId: string;
  count: number;
  expanded: boolean;
  isMuted: boolean;
  /** A hidden thread is marked unread with nothing to count. */
  marked: boolean;
  mentions: number;
  onToggle: (channelId: string) => void;
  unread: number;
  unreadOverflow: boolean;
}) {
  return (
    <SidebarMenuItem className="group/menu-item">
      <SidebarMenuButton
        aria-expanded={expanded}
        className="text-sidebar-foreground/60 group-hover/menu-item:bg-sidebar-accent group-hover/menu-item:text-sidebar-foreground"
        data-testid={`chat-more-threads-${channelId}`}
        onClick={() => onToggle(channelId)}
        type="button"
      >
        <span className="min-w-0 flex-1 truncate pl-6 text-xs">
          {expanded
            ? "Show less"
            : `+ ${count} more thread${count === 1 ? "" : "s"}`}
        </span>
        {mentions > 0 ? (
          <span
            className="ml-auto flex h-5 min-w-5 shrink-0 items-center justify-center rounded-full bg-sky-600 px-1 text-2xs font-semibold leading-none text-white"
            data-testid={`chat-more-threads-mentions-${channelId}`}
          >
            @
            <span className="sr-only">
              {" "}
              {mentions} unread mention{mentions === 1 ? "" : "s"}
            </span>
          </span>
        ) : null}
        {unread > 0 ? (
          <span
            className={cn(
              "flex h-5 min-w-5 shrink-0 items-center justify-center rounded-full px-1 text-2xs font-semibold leading-none tabular-nums",
              mentions > 0 ? "" : "ml-auto",
              isMuted
                ? "bg-sidebar-foreground/20 text-sidebar-foreground"
                : "bg-primary text-primary-foreground",
            )}
            data-testid={`chat-more-threads-unread-${channelId}`}
          >
            {formatChatCount(unread, unreadOverflow)}
            <span className="sr-only">
              {" "}
              unread message{unread === 1 ? "" : "s"}
            </span>
          </span>
        ) : marked && mentions === 0 ? (
          <span
            className="ml-auto h-2 w-2 shrink-0 rounded-full bg-primary"
            data-testid={`chat-more-threads-unread-dot-${channelId}`}
          >
            <span className="sr-only">unread</span>
          </span>
        ) : null}
      </SidebarMenuButton>
    </SidebarMenuItem>
  );
}
