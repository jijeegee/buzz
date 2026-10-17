import type { LucideIcon } from "lucide-react";
import {
  BellOff,
  BellRing,
  CircleCheck,
  Ellipsis,
  Link,
  CircleDot,
  RotateCcw,
} from "lucide-react";
import type * as React from "react";

import { buildMessageLink } from "@/features/messages/lib/messageLink";
import { copyTextToClipboard } from "@/shared/lib/clipboard";
import { Button } from "@/shared/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/shared/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/shared/ui/tooltip";

function HeaderIconButton({
  disabled,
  icon: Icon,
  label,
  onClick,
  pressed,
  testId,
}: {
  disabled?: boolean;
  icon: LucideIcon;
  label: string;
  onClick: () => void;
  pressed?: boolean;
  testId: string;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          aria-label={label}
          aria-pressed={pressed}
          className="text-muted-foreground hover:text-foreground"
          data-testid={testId}
          disabled={disabled}
          onClick={onClick}
          size="icon-xs"
          type="button"
          variant="ghost"
        >
          <Icon />
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

/**
 * Thread header bar: Follow/Unfollow · Copy link · Close/Reopen · Goals ·
 * Show in channel · ⋯ (Mark as unread). The panel keeps its own ✕.
 */
export function ThreadHeaderActions({
  channelId,
  closePending,
  goalChip,
  isClosed,
  isFollowing,
  onFollow,
  onMarkUnread,
  onToggleClosed,
  onUnfollow,
  rootId,
  showInChannelButton,
}: {
  channelId: string | null;
  closePending?: boolean;
  goalChip?: React.ReactNode;
  isClosed: boolean;
  isFollowing?: boolean;
  onFollow?: () => void;
  onMarkUnread?: () => void;
  onToggleClosed?: () => void;
  onUnfollow?: () => void;
  rootId: string;
  showInChannelButton?: React.ReactNode;
}) {
  const canFollow = isFollowing ? Boolean(onUnfollow) : Boolean(onFollow);
  return (
    <>
      {canFollow ? (
        <HeaderIconButton
          icon={isFollowing ? BellOff : BellRing}
          label={isFollowing ? "Unfollow thread" : "Follow thread"}
          onClick={() => (isFollowing ? onUnfollow?.() : onFollow?.())}
          testId="message-thread-follow-toggle"
        />
      ) : null}
      {channelId ? (
        <HeaderIconButton
          icon={Link}
          label="Copy link"
          onClick={() =>
            copyTextToClipboard(
              buildMessageLink({ channelId, messageId: rootId }),
              "Link copied to clipboard",
            )
          }
          testId="message-thread-copy-link"
        />
      ) : null}
      {onToggleClosed ? (
        <HeaderIconButton
          disabled={closePending}
          icon={isClosed ? RotateCcw : CircleCheck}
          label={isClosed ? "Reopen thread" : "Close thread"}
          onClick={onToggleClosed}
          pressed={isClosed}
          testId="message-thread-close-toggle"
        />
      ) : null}
      {goalChip}
      {showInChannelButton}
      {onMarkUnread ? (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              aria-label="More thread actions"
              className="text-muted-foreground hover:text-foreground"
              data-testid="message-thread-more"
              size="icon-xs"
              type="button"
              variant="ghost"
            >
              <Ellipsis />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem onSelect={onMarkUnread}>
              <CircleDot className="h-4 w-4" />
              Mark as unread
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      ) : null}
    </>
  );
}
