import type * as React from "react";

import type { ChatRowPreview } from "@/features/chats/chatRowPreview";
import {
  ConversationRowFrame,
  conversationRowPreviewClass,
} from "@/features/home/ui/ConversationRowFrame";
import { cn } from "@/shared/lib/cn";
import { formatItemTimestamp } from "@/shared/lib/datetime";
import { useChatRowDisplay } from "./chatRowDisplay";

/**
 * A Chats row with the Inbox row's look: avatar, name with the working
 * timer and the time on the first line, the latest message below in one
 * line, and the unread and @ badges under the time. No hover actions.
 */
export function ChatConversationRow({
  ariaLabel,
  avatar,
  badges,
  bold,
  buttonProps,
  dimmed,
  indent = false,
  isActive,
  onSelect,
  preview,
  title,
  titleExtras,
  working,
  ...rest
}: Omit<React.HTMLAttributes<HTMLDivElement>, "title"> & {
  ariaLabel: string;
  avatar: React.ReactNode;
  badges?: React.ReactNode;
  /** Unread: the latest message is shown bold. */
  bold: boolean;
  /** Test ids and data attributes for the row's button. */
  buttonProps?: React.ComponentProps<
    typeof ConversationRowFrame
  >["buttonProps"];
  dimmed: boolean;
  /** Set in By channel, where a thread sits under its channel. */
  indent?: boolean;
  isActive: boolean;
  onSelect: () => void;
  preview: ChatRowPreview;
  title: React.ReactNode;
  /** Small marks after the name (pin, mute). */
  titleExtras?: React.ReactNode;
  /** The working timer, left of the time. */
  working?: React.ReactNode;
  /** Lets an anchor (the activity popover) attach to the row. */
  ref?: React.Ref<HTMLDivElement>;
}) {
  const { compact } = useChatRowDisplay();
  return (
    <ConversationRowFrame
      {...rest}
      ariaLabel={ariaLabel}
      avatar={avatar}
      buttonProps={buttonProps}
      data-active={String(isActive)}
      className={cn(
        rest.className,
        dimmed && "sidebar-muted-content opacity-50 dark:opacity-45",
      )}
      contentClassName={cn(compact ? "py-1" : "py-2.5", indent && "pl-6")}
      isSelected={isActive}
      meta={
        <>
          {working}
          <span className="tabular-nums">
            {preview.at !== null ? formatItemTimestamp(preview.at) : ""}
          </span>
        </>
      }
      onSelect={onSelect}
      title={
        <span
          className="flex min-w-0 flex-1 items-center gap-1 text-sm font-semibold leading-4 text-foreground"
          data-sidebar-row-label
        >
          {title}
          {titleExtras}
        </span>
      }
    >
      {preview.line || badges ? (
        <div className="flex min-w-0 items-start gap-2">
          <p
            className={cn(
              conversationRowPreviewClass(bold),
              "min-w-0 flex-1 truncate",
            )}
          >
            {preview.line}
          </p>
          {badges ? (
            <span className="mt-1.5 flex shrink-0 items-center gap-1">
              {badges}
            </span>
          ) : null}
        </div>
      ) : null}
    </ConversationRowFrame>
  );
}
