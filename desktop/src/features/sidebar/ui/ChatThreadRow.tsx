import { CircleCheck, Pin } from "lucide-react";

import {
  chatThreadTitle,
  formatChatCount,
  type ChatListRow,
  type ChatRowUnread,
} from "@/features/chats/chatListState";
import type { ChatRowPreview } from "@/features/chats/chatRowPreview";
import { useT } from "@/shared/i18n";
import { cn } from "@/shared/lib/cn";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuTrigger,
} from "@/shared/ui/context-menu";
import { SidebarMenuItem } from "@/shared/ui/sidebar";
import { ConversationLetterAvatar } from "@/features/home/ui/ConversationLetterAvatar";
import type { ActiveChannelTurnSummary } from "@/features/agents/activeAgentTurnsStore";
import { ChatConversationRow } from "./ChatRowLines";
import { ChannelWorkingBadge } from "./SidebarSection";
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
  channelPicture = null,
  indent = false,
  isActive = false,
  isDm,
  isMuted,
  isPinned,
  onOpenThread,
  preview,
  row,
  unread,
  working,
}: {
  actions?: ChatThreadRowActions;
  channelLabel: string;
  /** The group photo, drawn on the channel circle of the avatar. */
  channelPicture?: string | null;
  /** Set in By channel, where the thread sits under its channel. */
  indent?: boolean;
  /** This thread is the one open now. */
  isActive?: boolean;
  isDm: boolean;
  isMuted: boolean;
  isPinned: boolean;
  onOpenThread: (channelId: string, rootId: string) => void;
  /** Latest reply and time for the second line. */
  preview: ChatRowPreview;
  row: ChatListRow;
  unread: ChatRowUnread;
  /** Agents working in this thread right now. */
  working?: ActiveChannelTurnSummary;
}) {
  const t = useT();
  const rootId = row.root_id ?? row.key;
  const title = chatThreadTitle(row);
  const place = isDm ? channelLabel : `${channelLabel}`;
  const hasUnread = unread.count > 0 || unread.marked;
  const dimmed = row.closed || (isMuted && unread.mentions === 0);
  const badges = (
    <>
      {row.closed ? (
        <span className="shrink-0" data-testid={`chat-thread-closed-${rootId}`}>
          <CircleCheck
            aria-hidden="true"
            className="h-3.5 w-3.5 text-sidebar-foreground/50"
          />
          <span className="sr-only">{t("sidebar.threadRow.closedSr")}</span>
        </span>
      ) : null}
      {unread.mentions > 0 ? (
        <span
          className="flex h-5 min-w-5 shrink-0 items-center justify-center rounded-full bg-sky-600 px-1 text-2xs font-semibold leading-none text-white"
          data-testid={`chat-thread-mentions-${rootId}`}
        >
          @
          <span className="sr-only">
            {" "}
            {t("sidebar.threadRow.mentionsSr", {
              count: unread.mentions,
              shown: formatChatCount(unread.mentions, unread.mentionsOverflow),
            })}
          </span>
        </span>
      ) : null}
      {unread.count > 0 ? (
        <span
          className={cn(
            "flex h-5 min-w-5 shrink-0 items-center justify-center rounded-full px-1 text-2xs font-semibold leading-none tabular-nums",
            isMuted
              ? "bg-sidebar-foreground/20 text-sidebar-foreground"
              : "bg-primary text-primary-foreground",
          )}
          data-testid={`chat-thread-unread-${rootId}`}
        >
          {formatChatCount(unread.count, unread.overflow)}
          <span className="sr-only">
            {" "}
            {t("sidebar.moreThreads.unreadSr", { count: unread.count })}
          </span>
        </span>
      ) : unread.marked ? (
        <span
          className="h-2 w-2 shrink-0 rounded-full bg-primary"
          data-testid={`chat-thread-unread-dot-${rootId}`}
        >
          <span className="sr-only">{t("sidebar.unreadSr")}</span>
        </span>
      ) : null}
    </>
  );
  const item = (
    <SidebarMenuItem className="group/menu-item">
      <ChatConversationRow
        ariaLabel={`${place} › ${title}`}
        avatar={
          <ConversationLetterAvatar
            channelId={row.channel_id}
            channelLabel={channelLabel}
            isThread
            picture={channelPicture}
            testId={`chat-thread-avatar-${rootId}`}
            thread={{ id: rootId, label: title }}
          />
        }
        badges={badges}
        bold={hasUnread && !dimmed}
        data-chat-thread-id={rootId}
        data-testid={`chat-thread-${rootId}`}
        dimmed={dimmed}
        indent={indent}
        isActive={isActive}
        onSelect={() => onOpenThread(row.channel_id, rootId)}
        preview={preview}
        title={
          <>
            <span className="max-w-[45%] shrink-0 truncate font-normal text-muted-foreground">
              {place}
            </span>
            <span
              aria-hidden="true"
              className="shrink-0 font-normal text-muted-foreground/60"
            >
              ›
            </span>
            <span className="sr-only">{t("sidebar.threadRow.threadSr")}</span>
            <span className="min-w-0 truncate">{title}</span>
          </>
        }
        titleExtras={
          isPinned ? (
            <span className="shrink-0">
              <Pin aria-hidden="true" className="h-3 w-3" />
              <span className="sr-only">{t("sidebar.pinnedForYouSr")}</span>
            </span>
          ) : null
        }
        working={
          working ? (
            <ChannelWorkingBadge
              channelName={`thread-${rootId}`}
              isActive={isActive}
              summary={working}
            />
          ) : null
        }
      />
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
