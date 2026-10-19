import { Lock, MessagesSquare, Users } from "lucide-react";

import { cn } from "@/shared/lib/cn";

export function conversationHue(channelId: string) {
  let hash = 0;
  for (const char of channelId) {
    hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  }
  return hash % 360;
}

function initialOf(label: string) {
  return Array.from(label.trim())[0]?.toUpperCase() ?? "";
}

function conversationColor(key: string) {
  return `hsl(${conversationHue(key)} 55% 48%)`;
}

/**
 * A channel's picture: its first letter on a color picked from its id. A
 * thread adds a small speech-bubble badge, or, given `thread`, a second
 * circle with the thread's own letter in front of a smaller channel circle,
 * so threads of one channel look different. A private group adds a small
 * lock badge. Shared by Inbox and Chats rows.
 */
export function ConversationLetterAvatar({
  channelId,
  channelLabel,
  isPrivate = false,
  isThread,
  testId = "home-inbox-conversation-avatar",
  thread,
}: {
  channelId: string;
  channelLabel: string;
  isPrivate?: boolean;
  isThread: boolean;
  testId?: string;
  thread?: { id: string; label: string };
}) {
  const initial = initialOf(channelLabel);
  const background = conversationColor(channelId || channelLabel);
  if (isThread && thread) {
    return (
      <span
        aria-hidden="true"
        // Wider than a plain avatar; the negative margin keeps the row text
        // lined up with the other rows.
        className="relative -mr-2 h-9 w-11 shrink-0"
        data-testid={testId}
      >
        <span
          className="absolute left-0 top-1/2 flex h-7 w-7 -translate-y-1/2 items-center justify-start rounded-full pl-2 text-2xs font-semibold text-white"
          style={{ backgroundColor: background }}
        >
          {initial || <Users className="h-3 w-3" />}
        </span>
        <span
          className="absolute right-0 top-1/2 flex h-7 w-7 -translate-y-1/2 items-center justify-center rounded-full text-xs font-semibold text-white ring-2 ring-background"
          style={{
            backgroundColor: conversationColor(thread.id || thread.label),
          }}
        >
          {initialOf(thread.label) || <MessagesSquare className="h-3 w-3" />}
        </span>
      </span>
    );
  }
  return (
    <span
      aria-hidden="true"
      className="relative flex h-9 w-9 shrink-0 items-center justify-center rounded-full text-sm font-semibold text-white"
      data-testid={testId}
      style={{ backgroundColor: background }}
    >
      {initial || <Users className="h-4 w-4" />}
      {isThread ? (
        <span className="absolute -bottom-0.5 -right-0.5 flex h-4 w-4 items-center justify-center rounded-full bg-background text-muted-foreground ring-1 ring-border">
          <MessagesSquare className="h-2.5 w-2.5" />
        </span>
      ) : isPrivate ? (
        <span
          className="absolute -bottom-0.5 -right-0.5 flex h-4 w-4 items-center justify-center rounded-full bg-background text-muted-foreground ring-1 ring-border"
          data-testid={`${testId}-lock`}
        >
          <Lock className="h-2.5 w-2.5" />
        </span>
      ) : null}
    </span>
  );
}

type GroupAvatarSize = "xs" | "sm" | "md" | "xl";

const GROUP_AVATAR_SIZES: Record<
  GroupAvatarSize,
  { box: string; text: string; icon: string; badge: string; lock: string }
> = {
  xs: {
    box: "size-4",
    text: "text-3xs",
    icon: "size-2.5",
    badge: "-bottom-1 -right-1 size-2.5",
    lock: "size-1.5",
  },
  sm: {
    box: "size-5",
    text: "text-2xs",
    icon: "size-3",
    badge: "-bottom-1 -right-1 size-3",
    lock: "size-2",
  },
  md: {
    box: "size-7",
    text: "text-xs",
    icon: "size-3.5",
    badge: "-bottom-0.5 -right-0.5 size-3.5",
    lock: "size-2",
  },
  xl: {
    box: "size-20",
    text: "text-3xl",
    icon: "size-8",
    badge: "bottom-0 right-0 size-6",
    lock: "size-3.5",
  },
};

/**
 * Small group picture for icon slots (menus, pills, headers): the same
 * first-letter-on-color as {@link ConversationLetterAvatar}, with a lock badge
 * on private groups. Without a name it falls back to a people icon.
 */
export function GroupLetterAvatar({
  channelId,
  className,
  isPrivate = false,
  label,
  size = "xs",
  testId,
}: {
  channelId: string;
  className?: string;
  isPrivate?: boolean;
  label?: string | null;
  size?: GroupAvatarSize;
  testId?: string;
}) {
  const dims = GROUP_AVATAR_SIZES[size];
  const initial = initialOf(label ?? "");
  return (
    <span
      aria-hidden="true"
      className={cn(
        "relative inline-flex shrink-0 items-center justify-center rounded-full font-semibold leading-none text-white",
        dims.box,
        dims.text,
        className,
      )}
      data-group-avatar=""
      data-testid={testId}
      style={{ backgroundColor: conversationColor(channelId || label || "") }}
    >
      {initial || <Users className={dims.icon} />}
      {isPrivate ? (
        <span
          className={cn(
            "absolute flex items-center justify-center rounded-full bg-background text-muted-foreground ring-1 ring-border",
            dims.badge,
          )}
          data-group-avatar-lock=""
        >
          <Lock className={dims.lock} />
        </span>
      ) : null}
    </span>
  );
}
