import {
  AlertCircle,
  Bell,
  Clock,
  Ellipsis,
  ExternalLink,
  LoaderCircle,
  MailOpen,
} from "lucide-react";
import * as React from "react";

import {
  getInboxThreadRootId,
  getInboxTypeLabel,
  type InboxFilter,
  type InboxItem,
  type InboxTypeLabel,
} from "@/features/home/lib/inbox";
import {
  formatInboxConversationRoomTitle,
  type InboxConversationRoom,
  resolveInboxConversationRoom,
} from "@/features/home/lib/inboxConversationRoom";
import { buildInboxListRows } from "@/features/home/lib/inboxListRows";
import { hasRenderedVideoAttachment } from "@/features/messages/lib/videoReviewContext";
import { getThreadReference } from "@/features/messages/lib/threading";
import { useThreadNameLabel } from "@/features/messages/lib/useThreadName";
import { ConversationRowFrame } from "@/features/home/ui/ConversationRowFrame";
import { InboxFilterMenu } from "@/features/home/ui/InboxFilterMenu";
import {
  DraftsPanel,
  type DraftViewItem,
} from "@/features/messages/ui/DraftsPanel";
import { UserProfilePopover } from "@/features/profile/ui/UserProfilePopover";
import type { Reminder } from "@/features/reminders/lib/reminderTypes";
import { isDue } from "@/features/reminders/lib/reminderFilters";
import {
  RemindersPanel,
  useReminderSources,
} from "@/features/reminders/ui/RemindersPanel";
import { type MessageKey, type Translate, useT } from "@/shared/i18n";
import { TopChromeInsetHeader } from "@/shared/layout/TopChromeInsetHeader";
import { cn } from "@/shared/lib/cn";
import { useChannelNavigation } from "@/shared/context/ChannelNavigationContext";
import type { SidebarDmParticipant } from "@/features/sidebar/ui/SidebarSection";
import { normalizePubkey } from "@/shared/lib/pubkey";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/shared/ui/context-menu";
import { VideoReviewCommentMarkdown } from "@/shared/ui/VideoReviewCommentMarkdown";
import {
  MENTION_CHIP_BASE_CLASSES,
  MESSAGE_MARKDOWN_CLASS,
} from "@/shared/ui/mentionChip";
import { Popover, PopoverContent, PopoverTrigger } from "@/shared/ui/popover";
import { Separator } from "@/shared/ui/separator";
import { Switch } from "@/shared/ui/switch";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/shared/ui/tooltip";
import { UserAvatar } from "@/shared/ui/UserAvatar";
import { ConversationLetterAvatar } from "./ConversationLetterAvatar";
import { ChannelChatAvatar } from "@/features/channels/ui/ChannelChatAvatar";
import { VirtualizedList } from "@/shared/ui/VirtualizedList";

const INBOX_EMPTY_STATE_TITLES: Record<InboxFilter, MessageKey> = {
  all: "home.inboxList.empty.noActivity",
  project: "home.inboxList.empty.noProjectWork",
  mention: "home.inboxList.empty.noMentions",
  thread: "home.inboxList.empty.noThreads",
  conversations: "home.inboxList.empty.noConversations",
  needs_action: "home.inboxList.empty.nothingNeedsAction",
  agent_activity: "home.inboxList.empty.noAgentUpdates",
  reminders: "home.inboxList.empty.noReminders",
  drafts: "home.inboxList.empty.noDrafts",
};

const INBOX_UNREAD_EMPTY_STATE_TITLES: Record<InboxFilter, MessageKey> = {
  all: "home.inboxList.empty.noUnreadActivity",
  project: "home.inboxList.empty.noUnreadProjectWork",
  mention: "home.inboxList.empty.noUnreadMentions",
  thread: "home.inboxList.empty.noUnreadThreads",
  conversations: "home.inboxList.empty.noUnreadConversations",
  needs_action: "home.inboxList.empty.noUnreadNeedsAction",
  agent_activity: "home.inboxList.empty.noUnreadAgentUpdates",
  reminders: "home.inboxList.empty.noUnreadReminders",
  drafts: "home.inboxList.empty.noUnreadDrafts",
};

const INBOX_HEADER_ICON_BUTTON_CLASS =
  "inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-muted/70 hover:text-foreground focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring data-[state=open]:bg-muted/70 data-[state=open]:text-foreground disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0";
const INBOX_PANE_RIGHT_DIVIDER_CLASS =
  "after:pointer-events-none after:absolute after:inset-y-0 after:right-0 after:z-40 after:w-px after:bg-border/35 after:content-['']";

function InboxLabel({
  channelId,
  isDone,
  isActionRequired,
  label,
  threadId,
}: {
  channelId?: string | null;
  isDone: boolean;
  isActionRequired: boolean;
  label: InboxTypeLabel;
  threadId?: string | null;
}) {
  const t = useT();
  const threadName = useThreadNameLabel(channelId, threadId);
  return (
    <div
      className={cn(
        MESSAGE_MARKDOWN_CLASS,
        "mt-0 flex min-h-[var(--inline-chip-min-height)] min-w-0 items-center gap-1.5 text-2xs leading-3 group-hover/inbox-item:pr-[6.75rem] group-focus-within/inbox-item:pr-[6.75rem]",
        isActionRequired && !isDone
          ? "font-medium text-amber-600/80 dark:text-amber-300/80"
          : isDone
            ? "font-normal text-muted-foreground/70"
            : "font-medium text-muted-foreground/80",
      )}
      data-inbox-type-label=""
    >
      <span className="shrink-0">{label.text}</span>
      {label.channelLabel ? (
        <span
          className={cn(
            MENTION_CHIP_BASE_CLASSES,
            "inbox-channel-chip min-w-0 max-w-full overflow-hidden",
          )}
          data-channel-link=""
        >
          <span className="truncate">
            {label.channelLabel}
            {threadId ? ` › ${threadName || t("home.untitledThread")}` : null}
          </span>
        </span>
      ) : null}
    </div>
  );
}

/** Stable per-channel hue so each room keeps the same avatar color. */
function ConversationRoomAvatar({
  room,
  testId,
}: {
  room: InboxConversationRoom;
  testId: string;
}) {
  const { channels } = useChannelNavigation();
  const roomChannel =
    room.kind === "dm"
      ? null
      : (channels.find((channel) => channel.id === room.channelId) ?? null);
  const channelPicture = roomChannel?.picture ?? null;
  if (roomChannel?.groupChat) {
    // An ownerless chat's picture is its people.
    return (
      <ChannelChatAvatar
        channel={roomChannel}
        className="size-9"
        testId={testId}
      />
    );
  }
  if (room.kind === "dm") {
    // A DM room's picture is the other participant's profile.
    const { person } = room;
    return (
      <div className="relative shrink-0" data-inbox-profile-trigger="true">
        <UserProfilePopover
          botIdenticonValue={person.label}
          pubkey={person.pubkey}
          role={person.isAgent ? "bot" : undefined}
          triggerClassName={cn(
            "shrink-0 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring",
            person.isAgent ? "rounded-[30%]" : "rounded-full",
          )}
          triggerElement="span"
          triggerTestId={testId}
        >
          <span className="inline-flex shrink-0">
            <UserAvatar
              avatarUrl={person.avatarUrl}
              className="h-9 w-9"
              displayName={person.label}
              shape={person.isAgent ? "squircle" : "circle"}
              size="md"
            />
          </span>
        </UserProfilePopover>
      </div>
    );
  }

  // Channel and thread rooms share the channel's letter and color; a thread
  // adds a badge.
  return (
    <ConversationLetterAvatar
      channelId={room.channelId}
      channelLabel={room.channelLabel}
      isThread={room.kind === "thread"}
      picture={channelPicture}
    />
  );
}

function ConversationRoomTitle({ room }: { room: InboxConversationRoom }) {
  const threadName = useThreadNameLabel(
    room.kind === "thread" ? room.channelId : null,
    room.kind === "thread" ? room.threadId : null,
  );
  return (
    <span className="block min-w-0 flex-1 truncate text-sm font-semibold leading-4 text-foreground">
      {formatInboxConversationRoomTitle(room, threadName)}
    </span>
  );
}

function formatReminderStatus(t: Translate, notBefore: number | undefined) {
  if (notBefore === undefined) return t("home.reminderStatus.pending");
  const secondsUntil = notBefore - Math.floor(Date.now() / 1_000);
  if (secondsUntil <= 0) return t("home.reminderStatus.due");
  if (secondsUntil < 60) return t("home.reminderStatus.lessThanMinute");
  if (secondsUntil < 3_600) {
    return t("home.reminderStatus.inMinutes", {
      count: Math.floor(secondsUntil / 60),
    });
  }
  if (secondsUntil < 86_400) {
    return t("home.reminderStatus.inHours", {
      count: Math.floor(secondsUntil / 3_600),
    });
  }
  return t("home.reminderStatus.inDays", {
    count: Math.floor(secondsUntil / 86_400),
  });
}

function getInboxVideoReviewCommentRootId(item: InboxItem) {
  const feedItems = [item.item, ...item.groupItems];
  const feedItemById = new Map(
    feedItems.map((feedItem) => [feedItem.id, feedItem]),
  );
  const videoMessageIds = new Set(
    feedItems
      .filter((feedItem) =>
        hasRenderedVideoAttachment({
          body: feedItem.content,
          tags: feedItem.tags,
        }),
      )
      .map((feedItem) => feedItem.id),
  );
  const visited = new Set<string>();
  let ancestorId = getThreadReference(item.item.tags).parentId;

  while (ancestorId && !visited.has(ancestorId)) {
    if (videoMessageIds.has(ancestorId)) return ancestorId;
    visited.add(ancestorId);
    const ancestor = feedItemById.get(ancestorId);
    ancestorId = ancestor ? getThreadReference(ancestor.tags).parentId : null;
  }

  return undefined;
}

function PersonalItemRow({
  id,
  location,
  onClick,
  preview,
  selected,
  status,
}: {
  id: string;
  location: InboxTypeLabel | null;
  onClick: () => void;
  preview: string;
  selected: boolean;
  status: string;
}) {
  const t = useT();
  return (
    <button
      aria-current={selected ? "true" : undefined}
      className={cn(
        "flex w-full items-center gap-3 border-b border-border/45 px-4 py-3 text-left transition-colors hover:bg-muted/40 focus-visible:bg-muted/40 focus-visible:outline-hidden",
        selected && "bg-muted/40",
      )}
      data-testid={`home-all-reminders-${id}`}
      onClick={onClick}
      type="button"
    >
      <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full bg-muted text-muted-foreground">
        <Bell className="h-4 w-4" />
      </span>
      <span className="min-w-0 flex-1">
        <span className="block text-sm font-semibold text-foreground">
          {t("home.itemKind.reminder")}
        </span>
        {location ? (
          <InboxLabel
            isActionRequired={false}
            isDone={false}
            label={location}
          />
        ) : null}
        <span className="block truncate text-sm text-muted-foreground">
          {preview}
        </span>
      </span>
      <span className="shrink-0 text-xs font-medium text-muted-foreground">
        {status}
      </span>
    </button>
  );
}

type InboxListPaneProps = {
  activeReminderEventIds?: ReadonlySet<string>;
  agentPubkeys?: ReadonlySet<string>;
  /** DM room labels and counterparts, keyed by channel id. */
  dmChannelLabels?: Readonly<Record<string, string>>;
  dmParticipantsByChannelId?: Readonly<Record<string, SidebarDmParticipant[]>>;
  activeDraftCount: number;
  draftItems: DraftViewItem[];
  doneSet: ReadonlySet<string>;
  filter: InboxFilter;
  items: InboxItem[];
  onFilterChange: (filter: InboxFilter) => void;
  onDeleteDraft: (draftKey: string) => void;
  onMarkRead: (itemId: string) => void;
  onMarkUnread: (itemId: string) => void;
  onOpenDirect: (item: InboxItem) => void;
  isReopenPending?: (channelId: string | null | undefined) => boolean;
  isReopenErrored?: (channelId: string | null | undefined) => boolean;
  onRemindLater: (item: InboxItem) => void;
  onSelect: (itemId: string) => void;
  onSelectDraft: (draftKey: string) => void;
  onSelectReminder: (reminderId: string) => void;
  onUnreadOnlyChange: (checked: boolean) => void;
  selectedConversationId: string | null;
  selectedDraftKey: string | null;
  showRightDivider?: boolean;
  dueReminderCount: number;
  reminderPubkey?: string;
  reminders: readonly Reminder[];
  selectedReminderId: string | null;
  unreadOnly: boolean;
};

export function InboxListPane({
  activeReminderEventIds,
  agentPubkeys,
  dmChannelLabels,
  dmParticipantsByChannelId,
  activeDraftCount,
  draftItems,
  doneSet,
  filter,
  items,
  onFilterChange,
  onDeleteDraft,
  onMarkRead,
  onMarkUnread,
  onOpenDirect,
  isReopenPending,
  isReopenErrored,
  onRemindLater,
  onSelect,
  onSelectDraft,
  onSelectReminder,
  onUnreadOnlyChange,
  selectedConversationId,
  selectedDraftKey,
  showRightDivider = false,
  dueReminderCount,
  reminderPubkey,
  reminders,
  selectedReminderId,
  unreadOnly,
}: InboxListPaneProps) {
  const t = useT();
  const isReminders = filter === "reminders";
  const isDrafts = filter === "drafts";
  const isMixedInboxView = filter === "all";
  // Chat-list presentation: the conversation, not the sender, leads the row.
  const isConversationView = filter === "conversations";
  const scrollRef = React.useRef<HTMLDivElement>(null);
  const inboxRows = React.useMemo(
    () =>
      buildInboxListRows({
        items,
        reminders: unreadOnly
          ? []
          : reminders.filter((reminder) =>
              isDue(reminder, Math.floor(Date.now() / 1_000)),
            ),
      }),
    [items, reminders, unreadOnly],
  );
  const visibleInboxRows = React.useMemo(
    () =>
      isMixedInboxView
        ? inboxRows
        : inboxRows.filter((row) => row.kind === "inbox"),
    [inboxRows, isMixedInboxView],
  );
  const reminderSources = useReminderSources(reminders);
  const unreadVisibleItemCount = React.useMemo(
    () =>
      items.reduce((count, item) => count + (doneSet.has(item.id) ? 0 : 1), 0),
    [doneSet, items],
  );
  const handleMarkAllRead = React.useCallback(() => {
    for (const item of items) {
      if (!doneSet.has(item.id)) {
        onMarkRead(item.id);
      }
    }
  }, [doneSet, items, onMarkRead]);

  const renderItem = (item: InboxItem, dueReminder?: Reminder) => {
    const isSelected = item.conversationId === selectedConversationId;
    const isDone = doneSet.has(item.id);
    const hasActiveReminder =
      dueReminder !== undefined ||
      [item.id, ...item.groupItems.map((groupItem) => groupItem.id)].some(
        (eventId) => activeReminderEventIds?.has(eventId) ?? false,
      );
    const hasChannelTarget = Boolean(item.item.channelId);
    const isReopening = isReopenPending?.(item.item.channelId) ?? false;
    const hasReopenError = isReopenErrored?.(item.item.channelId) ?? false;
    const canOpen = hasChannelTarget && !isReopening;
    const openLabel = !hasChannelTarget
      ? t("home.inboxRow.noGroupLink")
      : isReopening
        ? t("home.inboxRow.reopening")
        : t("home.inboxRow.openInGroup");
    const typeLabel = getInboxTypeLabel(item);
    const threadRootId = getInboxThreadRootId(item);
    const videoReviewCommentRootId = getInboxVideoReviewCommentRootId(item);
    const isSenderAgent =
      agentPubkeys?.has(normalizePubkey(item.item.pubkey)) === true;
    const profileRole = isSenderAgent ? "bot" : undefined;
    // In the conversations view the row is the chat room, not the message.
    const conversationRoom = isConversationView
      ? resolveInboxConversationRoom(item, {
          dmChannelLabels,
          dmParticipantsByChannelId,
          isAgentPubkey: (pubkey) =>
            agentPubkeys?.has(normalizePubkey(pubkey)) === true,
        })
      : null;
    const handleRowContentClick = (event: React.MouseEvent<HTMLElement>) => {
      const target = event.target;
      if (
        target instanceof Element &&
        target.closest("[data-inbox-profile-trigger]")
      ) {
        return;
      }
      onSelect(item.id);
    };
    const row = (
      <ConversationRowFrame
        ariaLabel={t("home.inboxRow.openItemFrom", {
          name: item.senderLabel,
        })}
        avatar={
          conversationRoom ? (
            <ConversationRoomAvatar
              room={conversationRoom}
              testId={`home-inbox-avatar-${item.id}`}
            />
          ) : (
            <div
              className="relative shrink-0"
              data-inbox-profile-trigger="true"
            >
              <UserProfilePopover
                botIdenticonValue={item.senderLabel}
                pubkey={item.item.pubkey}
                role={profileRole}
                triggerClassName={cn(
                  "shrink-0 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring",
                  isSenderAgent ? "rounded-[30%]" : "rounded-full",
                )}
                triggerElement="span"
                triggerTestId={`home-inbox-avatar-${item.id}`}
              >
                <span className="inline-flex shrink-0">
                  <UserAvatar
                    avatarUrl={item.avatarUrl}
                    className="h-9 w-9"
                    displayName={item.senderLabel}
                    shape={isSenderAgent ? "squircle" : "circle"}
                    size="md"
                  />
                </span>
              </UserProfilePopover>
            </div>
          )
        }
        data-testid={`home-inbox-item-${item.id}`}
        hideMetaOnHover
        hoverActions={
          <>
            {isDone ? (
              <InboxRowActionButton
                label={t("home.inboxRow.markUnread")}
                onClick={() => onMarkUnread(item.id)}
              >
                <MailOpen className="!h-4 !w-4" />
              </InboxRowActionButton>
            ) : (
              <InboxRowActionButton
                label={t("home.inboxRow.markRead")}
                onClick={() => onMarkRead(item.id)}
              >
                <MailOpen className="!h-4 !w-4" />
              </InboxRowActionButton>
            )}
            <InboxRowActionButton
              disabled={!canOpen}
              label={openLabel}
              onClick={() => onOpenDirect(item)}
            >
              <ExternalLink className="!h-4 !w-4" />
            </InboxRowActionButton>
            <InboxRowActionButton
              active={hasActiveReminder}
              disabled={!hasChannelTarget}
              label={
                hasChannelTarget
                  ? hasActiveReminder
                    ? t("home.inboxRow.reminderSet")
                    : t("home.inboxRow.remindLater")
                  : t("home.inboxRow.cannotRemind")
              }
              onClick={() => onRemindLater(item)}
            >
              <Clock className="!h-4 !w-4" />
            </InboxRowActionButton>
          </>
        }
        isSelected={isSelected}
        meta={
          <>
            {!isDone ? (
              <span
                aria-hidden="true"
                className="h-1.5 w-1.5 rounded-full bg-primary"
              />
            ) : null}
            {item.unreadCount > 1 ? (
              <span data-testid="home-inbox-unread-count">
                {t("home.inboxRow.unreadCount", { count: item.unreadCount })}
              </span>
            ) : null}
            {item.timestampLabel}
          </>
        }
        metaClassName={isDone ? "font-normal" : "font-medium"}
        onContentClick={handleRowContentClick}
        onSelect={() => onSelect(item.id)}
        title={
          conversationRoom ? (
            <ConversationRoomTitle room={conversationRoom} />
          ) : (
            <span
              className="flex min-w-0 flex-1 items-start leading-4"
              data-inbox-profile-trigger="true"
            >
              <UserProfilePopover
                botIdenticonValue={item.senderLabel}
                pubkey={item.item.pubkey}
                role={profileRole}
                triggerElement="span"
              >
                <span className="block max-w-full truncate rounded text-sm font-semibold leading-4 text-foreground focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring">
                  {item.senderLabel}
                </span>
              </UserProfilePopover>
            </span>
          )
        }
      >
        {conversationRoom ? null : (
          <InboxLabel
            channelId={item.item.channelId}
            isActionRequired={item.isActionRequired}
            isDone={isDone}
            label={typeLabel}
            threadId={threadRootId}
          />
        )}
        {dueReminder ? (
          <div
            className="mt-1 flex items-center gap-1 text-2xs font-medium text-amber-600/80 dark:text-amber-300/80"
            data-testid="home-inbox-reminder-due"
          >
            <Bell className="h-3 w-3" />
            {t("home.reminderStatus.due")}
          </div>
        ) : null}

        {isReopening || hasReopenError ? (
          <div
            aria-live="polite"
            className={cn(
              "mt-1 flex items-center gap-1 text-2xs font-medium",
              hasReopenError ? "text-destructive" : "text-muted-foreground",
            )}
            data-testid={`home-inbox-reopen-status-${item.id}`}
            role="status"
          >
            {isReopening ? (
              <>
                <LoaderCircle className="h-3 w-3 shrink-0 animate-spin" />
                {t("home.inboxRow.reopening")}
              </>
            ) : (
              <>
                <AlertCircle className="h-3 w-3 shrink-0" />
                {t("home.inboxRow.reopenFailed")}
                {canOpen ? (
                  <button
                    className="ml-0.5 rounded font-semibold underline underline-offset-2 hover:no-underline focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
                    data-testid={`home-inbox-reopen-retry-${item.id}`}
                    onClick={(event) => {
                      // The row wrapper selects on click; stop it so
                      // Retry only re-issues the reopen for this row.
                      event.stopPropagation();
                      onOpenDirect(item);
                    }}
                    type="button"
                  >
                    {t("common.retry")}
                  </button>
                ) : null}
              </>
            )}
          </div>
        ) : null}

        <div
          className={cn(
            "mt-1.5 text-message [&_a]:font-medium [&_a]:text-current",
            isDone
              ? "font-normal text-muted-foreground"
              : "font-semibold text-foreground",
          )}
        >
          <VideoReviewCommentMarkdown
            className="inbox-preview-markdown text-inherit"
            content={item.preview}
            interactive={false}
            mentionNames={item.mentionNames}
            videoReviewCommentRootId={videoReviewCommentRootId}
          />
        </div>
      </ConversationRowFrame>
    );

    return (
      <ContextMenu>
        <ContextMenuTrigger asChild>{row}</ContextMenuTrigger>
        <ContextMenuContent>
          {isDone ? (
            <ContextMenuItem onClick={() => onMarkUnread(item.id)}>
              <MailOpen className="h-4 w-4" />
              {t("home.inboxRow.markUnread")}
            </ContextMenuItem>
          ) : (
            <ContextMenuItem onClick={() => onMarkRead(item.id)}>
              <MailOpen className="h-4 w-4" />
              {t("home.inboxRow.markRead")}
            </ContextMenuItem>
          )}
          <ContextMenuSeparator />
          <ContextMenuItem
            disabled={!canOpen}
            onClick={() => {
              if (canOpen) {
                onOpenDirect(item);
              }
            }}
          >
            <ExternalLink className="h-4 w-4" />
            {openLabel}
          </ContextMenuItem>
          <ContextMenuItem
            disabled={!hasChannelTarget}
            onClick={() => {
              if (hasChannelTarget) {
                onRemindLater(item);
              }
            }}
          >
            <Clock className="h-4 w-4" />
            {hasActiveReminder
              ? t("home.inboxRow.reminderSet")
              : t("home.inboxRow.remindLater")}
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>
    );
  };

  return (
    <section
      className={cn(
        "relative flex min-h-0 min-w-0 flex-col overflow-hidden bg-background/60",
        showRightDivider && INBOX_PANE_RIGHT_DIVIDER_CLASS,
      )}
    >
      <TopChromeInsetHeader flush transparent>
        <div className="px-5 py-2">
          <div className="flex min-h-9 w-full min-w-0 items-center justify-between gap-3">
            <div className="order-2 ml-auto flex shrink-0 items-center justify-end">
              <Popover>
                <PopoverTrigger asChild>
                  <button
                    aria-label={t("home.inboxList.options")}
                    className={cn(INBOX_HEADER_ICON_BUTTON_CLASS, "-mr-4")}
                    data-testid="inbox-options-trigger"
                    type="button"
                  >
                    <Ellipsis className="h-4 w-4" />
                  </button>
                </PopoverTrigger>
                <PopoverContent align="end" className="w-60 p-2">
                  <div
                    className={cn(
                      "flex min-h-9 items-center justify-between gap-3 rounded-lg px-2 py-1.5",
                      (isReminders || isDrafts) && "opacity-50",
                    )}
                  >
                    <label
                      className="text-sm font-medium text-foreground"
                      htmlFor="inbox-unread-only-switch"
                    >
                      {t("home.inboxList.showUnreadOnly")}
                    </label>
                    <Switch
                      checked={unreadOnly}
                      className="shadow-none [&>span]:shadow-none"
                      data-testid="inbox-unread-only-toggle"
                      disabled={isReminders || isDrafts}
                      id="inbox-unread-only-switch"
                      onCheckedChange={onUnreadOnlyChange}
                    />
                  </div>
                  <Separator className="my-1 bg-muted" />
                  <button
                    className="flex min-h-9 w-full items-center rounded-lg px-2 py-2 text-left text-sm transition-colors hover:bg-muted/50 disabled:pointer-events-none disabled:opacity-50"
                    disabled={unreadVisibleItemCount === 0}
                    onClick={handleMarkAllRead}
                    type="button"
                  >
                    <span>{t("home.inboxList.markAllRead")}</span>
                    {unreadVisibleItemCount > 0 ? (
                      <span className="ml-auto text-xs text-muted-foreground">
                        {unreadVisibleItemCount}
                      </span>
                    ) : null}
                  </button>
                </PopoverContent>
              </Popover>
            </div>
            <div className="order-1 flex shrink-0 items-center justify-start">
              <InboxFilterMenu
                activeDraftCount={activeDraftCount}
                dueReminderCount={dueReminderCount}
                filter={filter}
                onFilterChange={onFilterChange}
                reminderCount={reminders.length}
              />
            </div>
          </div>
        </div>
      </TopChromeInsetHeader>

      {isReminders ? (
        <div
          className="-mt-13 flex min-h-0 flex-1 flex-col overflow-hidden pt-13"
          data-testid="home-inbox-reminders"
        >
          {reminderPubkey ? (
            <RemindersPanel
              onSelectReminder={onSelectReminder}
              presentation="inbox-list"
              pubkey={reminderPubkey}
              selectedReminderId={selectedReminderId}
            />
          ) : null}
        </div>
      ) : isDrafts ? (
        <div
          className="-mt-13 flex min-h-0 flex-1 flex-col overflow-hidden pt-13"
          data-testid="home-inbox-drafts"
        >
          <DraftsPanel
            items={draftItems}
            onDeleteDraft={onDeleteDraft}
            onSelectDraft={onSelectDraft}
            selectedDraftKey={selectedDraftKey}
          />
        </div>
      ) : (
        <div
          className="-mt-13 min-h-0 flex-1 overflow-y-auto overflow-x-hidden overscroll-contain pt-13"
          data-testid="home-inbox-list"
          ref={scrollRef}
        >
          {visibleInboxRows.length > 0 ? (
            <VirtualizedList
              estimateSize={96}
              getItemKey={(row) => row.key}
              items={visibleInboxRows}
              renderItem={(row) => {
                if (row.kind === "inbox") {
                  return renderItem(row.item, row.dueReminder);
                }

                const source = reminderSources.get(row.reminder.id);
                return (
                  <PersonalItemRow
                    id={row.reminder.id}
                    location={
                      source?.channel
                        ? source.channel.channelType === "dm"
                          ? {
                              text: t("home.reminderRow.inDm", {
                                name: source.channelLabel,
                              }),
                              channelLabel: null,
                            }
                          : {
                              text: t("home.reminderRow.in"),
                              channelLabel: source.channelLabel,
                            }
                        : null
                    }
                    onClick={() => {
                      onSelectReminder(row.reminder.id);
                    }}
                    preview={
                      row.reminder.content.target?.preview ||
                      row.reminder.content.note ||
                      t("home.itemKind.reminder")
                    }
                    selected={selectedReminderId === row.reminder.id}
                    status={formatReminderStatus(t, row.reminder.notBefore)}
                  />
                );
              }}
              scrollRef={scrollRef}
            />
          ) : (
            <div className="flex h-full min-h-64 items-center justify-center px-6 text-center">
              <div>
                <p className="text-sm font-medium text-foreground">
                  {t(
                    unreadOnly
                      ? INBOX_UNREAD_EMPTY_STATE_TITLES[filter]
                      : INBOX_EMPTY_STATE_TITLES[filter],
                  )}
                </p>
                <p className="mt-1 text-sm text-muted-foreground">
                  {unreadOnly
                    ? t("home.inboxList.emptyHint.unreadOnly")
                    : filter === "all"
                      ? t("home.inboxList.emptyHint.all")
                      : t("home.inboxList.emptyHint.filtered")}
                </p>
              </div>
            </div>
          )}
        </div>
      )}
    </section>
  );
}

function InboxRowActionButton({
  active = false,
  children,
  disabled = false,
  label,
  onClick,
}: {
  active?: boolean;
  children: React.ReactNode;
  disabled?: boolean;
  label: string;
  onClick: () => void;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          aria-label={label}
          className={cn(
            "flex h-8 w-8 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-40",
            active && "bg-blue-500/10 text-blue-500 hover:text-blue-500",
          )}
          disabled={disabled}
          onClick={(event) => {
            event.preventDefault();
            event.stopPropagation();
            if (disabled) {
              return;
            }
            onClick();
          }}
          type="button"
        >
          {children}
        </button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}
