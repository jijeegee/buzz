import { summarizeMessageLinkContent } from "@/features/messages/lib/messageLinkMetadata";
import type { ChatListEvent } from "./chatListState";

/** The second line and time of a two-line Chats row. */
export type ChatRowPreview = {
  /** Latest message as one line, or null when the relay sent none. */
  line: string | null;
  /** Unix seconds of the row's activity, or null when unknown. */
  at: number | null;
};

const NO_TEXT = "No message text";

/**
 * The latest message as one line: `You: …` for my own message, `Name: …` in
 * channels and group DMs, the bare text in a one-to-one DM. A message with
 * only attachments reads "Photo".
 */
export function chatRowPreviewLine({
  authorLabel,
  event,
  isOneToOneDm,
  isSelf,
}: {
  authorLabel: string;
  event: ChatListEvent;
  isOneToOneDm: boolean;
  isSelf: boolean;
}): string {
  let text = summarizeMessageLinkContent(event.content);
  if (text === NO_TEXT && event.tags.some((tag) => tag[0] === "imeta")) {
    text = "Photo";
  }
  if (isSelf) return `You: ${text}`;
  return isOneToOneDm ? text : `${authorLabel}: ${text}`;
}
