import { useT } from "@/shared/i18n";
import { CircleCheck, RotateCcw } from "lucide-react";

import { hasThreadClosedTag } from "@/features/messages/lib/taskThread";
import type { TimelineMessage } from "@/features/messages/types";

/**
 * A past Close or Reopen marker reply, shown as a system line instead of a
 * bubble: "You closed this thread", "Mina reopened this thread".
 */
export function ThreadMarkerLine({
  currentPubkey,
  message,
}: {
  currentPubkey: string | undefined;
  message: Pick<TimelineMessage, "author" | "id" | "pubkey" | "tags">;
}) {
  const t = useT();
  const closed = hasThreadClosedTag(message.tags);
  const own =
    Boolean(currentPubkey) &&
    message.pubkey?.toLowerCase() === currentPubkey?.toLowerCase();
  const Icon = closed ? CircleCheck : RotateCcw;
  return (
    <div
      className="flex items-center justify-center gap-1.5 py-1.5 text-xs text-muted-foreground"
      data-message-id={message.id}
      data-testid="thread-marker-line"
    >
      <Icon aria-hidden className="size-3.5 shrink-0" />
      <span>
        {own
          ? closed
            ? t("messages.threadMarker.youClosed")
            : t("messages.threadMarker.youReopened")
          : closed
            ? t("messages.threadMarker.closed", { name: message.author })
            : t("messages.threadMarker.reopened", { name: message.author })}
      </span>
    </div>
  );
}
