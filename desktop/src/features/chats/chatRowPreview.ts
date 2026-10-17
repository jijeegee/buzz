import { summarizeMessageLinkContent } from "@/features/messages/lib/messageLinkMetadata";
import type { ChatListEvent } from "./chatListState";

/** The second line and time of a two-line Chats row. */
export type ChatRowPreview = {
  /** Latest message text, or null when the relay sent none. */
  line: string | null;
  /** Unix seconds of the row's activity, or null when unknown. */
  at: number | null;
};

const NO_TEXT = "No message text";

/**
 * The latest message's text alone, as other messengers show it: no author
 * name. A message with only attachments reads "Photo".
 */
export function chatRowPreviewLine(event: ChatListEvent): string {
  const text = summarizeMessageLinkContent(event.content);
  if (text === NO_TEXT && event.tags.some((tag) => tag[0] === "imeta")) {
    return "Photo";
  }
  return text;
}
