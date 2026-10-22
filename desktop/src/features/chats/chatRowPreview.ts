import { blockedMessageLabel } from "@/features/messages/lib/blockedMessage";
import { summarizeMessageLinkContent } from "@/features/messages/lib/messageLinkMetadata";
import { t } from "@/shared/i18n";
import type { ChatListEvent } from "./chatListState";

/** The second line and time of a two-line Chats row. */
export type ChatRowPreview = {
  /** Latest message text, or null when the relay sent none. */
  line: string | null;
  /** Unix seconds of the row's activity, or null when unknown. */
  at: number | null;
};

/**
 * The latest message's text alone, as other messengers show it: no author
 * name. A message with only attachments reads "Photo"; one from someone I
 * blocked never shows its text.
 */
export function chatRowPreviewLine(
  event: ChatListEvent,
  isBlocked: (pubkey: string) => boolean = () => false,
): string {
  if (isBlocked(event.pubkey)) return blockedMessageLabel();
  const text = summarizeMessageLinkContent(event.content);
  if (
    text === t("messages.linkPreview.noText") &&
    event.tags.some((tag) => tag[0] === "imeta")
  ) {
    return t("chats.rowPreview.photo");
  }
  return text;
}
