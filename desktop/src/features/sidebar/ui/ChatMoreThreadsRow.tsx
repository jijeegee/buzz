import { BY_CHANNEL_THREAD_STEP } from "@/features/chats/chatListGroups";
import { formatChatCount } from "@/features/chats/chatListState";
import { useT } from "@/shared/i18n";
import { cn } from "@/shared/lib/cn";
import { SidebarMenuButton, SidebarMenuItem } from "@/shared/ui/sidebar";

/**
 * By channel: "+ N more threads" under a channel's first threads, with the
 * hidden threads' summed unread and @, so hidden unread is never invisible.
 * A hidden thread marked unread with nothing to count shows a dot.
 * Each click shows 10 more: "+ 10 more · N left" (or "+ N more" for the last
 * ones) beside "Show less"; with every thread shown, only "Show less".
 */
export function ChatMoreThreadsRow({
  channelId,
  count,
  expanded,
  isMuted,
  marked,
  mentions,
  onShowLess,
  onShowMore,
  unread,
  unreadOverflow,
}: {
  channelId: string;
  /** Threads still hidden. */
  count: number;
  /** Shows threads past the first ones. */
  expanded: boolean;
  isMuted: boolean;
  /** A hidden thread is marked unread with nothing to count. */
  marked: boolean;
  mentions: number;
  onShowLess: (channelId: string) => void;
  onShowMore: (channelId: string) => void;
  unread: number;
  unreadOverflow: boolean;
}) {
  const t = useT();
  const showLess = expanded ? (
    <SidebarMenuButton
      className={cn(
        "text-sidebar-foreground/60",
        count > 0
          ? "w-auto shrink-0 hover:text-sidebar-foreground"
          : "group-hover/menu-item:bg-sidebar-accent group-hover/menu-item:text-sidebar-foreground",
      )}
      data-testid={`chat-more-threads-less-${channelId}`}
      onClick={() => onShowLess(channelId)}
      type="button"
    >
      <span
        className={cn(
          "truncate text-xs",
          count > 0 ? "" : "min-w-0 flex-1 pl-6",
        )}
      >
        {t("sidebar.moreThreads.showLess")}
      </span>
    </SidebarMenuButton>
  ) : null;
  if (count === 0) {
    return (
      <SidebarMenuItem className="group/menu-item">{showLess}</SidebarMenuItem>
    );
  }
  const label = !expanded
    ? t("sidebar.moreThreads.moreThreads", { count })
    : count > BY_CHANNEL_THREAD_STEP
      ? t("sidebar.moreThreads.nextStep", {
          step: BY_CHANNEL_THREAD_STEP,
          count,
        })
      : t("sidebar.moreThreads.lastStep", { count });
  return (
    <SidebarMenuItem
      className={cn("group/menu-item", expanded && "flex items-center gap-1")}
    >
      <SidebarMenuButton
        aria-expanded={expanded}
        className={cn(
          "text-sidebar-foreground/60",
          expanded
            ? "min-w-0 flex-1 hover:text-sidebar-foreground"
            : "group-hover/menu-item:bg-sidebar-accent group-hover/menu-item:text-sidebar-foreground",
        )}
        data-testid={`chat-more-threads-${channelId}`}
        onClick={() => onShowMore(channelId)}
        type="button"
      >
        <span className="min-w-0 flex-1 truncate pl-6 text-xs">{label}</span>
        {mentions > 0 ? (
          <span
            className="ml-auto flex h-5 min-w-5 shrink-0 items-center justify-center rounded-full bg-sky-600 px-1 text-2xs font-semibold leading-none text-white"
            data-testid={`chat-more-threads-mentions-${channelId}`}
          >
            @
            <span className="sr-only">
              {" "}
              {t("sidebar.moreThreads.mentionsSr", { count: mentions })}
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
              {t("sidebar.moreThreads.unreadSr", { count: unread })}
            </span>
          </span>
        ) : marked && mentions === 0 ? (
          <span
            className="ml-auto h-2 w-2 shrink-0 rounded-full bg-primary"
            data-testid={`chat-more-threads-unread-dot-${channelId}`}
          >
            <span className="sr-only">{t("sidebar.unreadSr")}</span>
          </span>
        ) : null}
      </SidebarMenuButton>
      {showLess}
    </SidebarMenuItem>
  );
}
