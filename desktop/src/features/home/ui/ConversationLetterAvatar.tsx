import { MessagesSquare } from "lucide-react";

function conversationHue(channelId: string) {
  let hash = 0;
  for (const char of channelId) {
    hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  }
  return hash % 360;
}

/**
 * A channel's picture: its first letter on a color picked from its id. A
 * thread adds a small speech-bubble badge. Shared by Inbox and Chats rows.
 */
export function ConversationLetterAvatar({
  channelId,
  channelLabel,
  isThread,
  testId = "home-inbox-conversation-avatar",
}: {
  channelId: string;
  channelLabel: string;
  isThread: boolean;
  testId?: string;
}) {
  const initial = Array.from(channelLabel)[0]?.toUpperCase() ?? "#";
  const hue = conversationHue(channelId || channelLabel);
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
