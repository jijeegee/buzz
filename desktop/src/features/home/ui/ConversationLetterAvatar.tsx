import { MessagesSquare } from "lucide-react";

function conversationHue(channelId: string) {
  let hash = 0;
  for (const char of channelId) {
    hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  }
  return hash % 360;
}

function initialOf(label: string) {
  return Array.from(label)[0]?.toUpperCase() ?? "#";
}

/**
 * A channel's picture: its first letter on a color picked from its id. A
 * thread adds a small speech-bubble badge, or, given `thread`, a second
 * circle with the thread's own letter in front of a smaller channel circle,
 * so threads of one channel look different. Shared by Inbox and Chats rows.
 */
export function ConversationLetterAvatar({
  channelId,
  channelLabel,
  isThread,
  testId = "home-inbox-conversation-avatar",
  thread,
}: {
  channelId: string;
  channelLabel: string;
  isThread: boolean;
  testId?: string;
  thread?: { id: string; label: string };
}) {
  const initial = initialOf(channelLabel);
  const hue = conversationHue(channelId || channelLabel);
  if (isThread && thread) {
    return (
      <span
        aria-hidden="true"
        className="relative h-9 w-9 shrink-0"
        data-testid={testId}
      >
        <span
          className="absolute left-0 top-0 flex h-[1.625rem] w-[1.625rem] items-start justify-start rounded-full pl-1.5 pt-0.5 text-[0.625rem] font-semibold text-white"
          style={{ backgroundColor: `hsl(${hue} 55% 48%)` }}
        >
          {initial}
        </span>
        <span
          className="absolute -bottom-0.5 -right-0.5 flex h-6 w-6 items-center justify-center rounded-full text-2xs font-semibold text-white ring-2 ring-background"
          style={{
            backgroundColor: `hsl(${conversationHue(thread.id || thread.label)} 55% 48%)`,
          }}
        >
          {initialOf(thread.label)}
        </span>
      </span>
    );
  }
  return (
    <span
      aria-hidden="true"
      className="relative flex h-9 w-9 shrink-0 items-center justify-center rounded-full text-sm font-semibold text-white"
      data-testid={testId}
      style={{ backgroundColor: `hsl(${hue} 55% 48%)` }}
    >
      {initial}
      {isThread ? (
        <span className="absolute -bottom-0.5 -right-0.5 flex h-4 w-4 items-center justify-center rounded-full bg-background text-muted-foreground ring-1 ring-border">
          <MessagesSquare className="h-2.5 w-2.5" />
        </span>
      ) : null}
    </span>
  );
}
