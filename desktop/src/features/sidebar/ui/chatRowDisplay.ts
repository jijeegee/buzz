import * as React from "react";

/**
 * How Chats rows draw in a list other than the sidebar's own: the owner
 * view uses tighter rows and a small tag after each chat's name.
 */
export type ChatRowDisplay = {
  compact: boolean;
  /** A short tag after a room's name ("agent", "person", "3 people"). */
  tagOf?: (channelId: string) => string | null;
};

const DEFAULT_DISPLAY: ChatRowDisplay = { compact: false };

export const ChatRowDisplayContext =
  React.createContext<ChatRowDisplay>(DEFAULT_DISPLAY);

export function useChatRowDisplay(): ChatRowDisplay {
  return React.useContext(ChatRowDisplayContext);
}
