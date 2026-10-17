import type * as React from "react";

import type { ChatRowPreview } from "@/features/chats/chatRowPreview";
import { cn } from "@/shared/lib/cn";
import { formatItemTimestamp } from "@/shared/lib/datetime";

/** A Chats row's button: tall enough for a name and two message lines. */
export const CHAT_ROW_BUTTON_CLASS =
  "h-auto min-h-[3.75rem] items-start gap-2.5 py-2.5 data-[active=true]:font-normal";

/**
 * The text of a Chats row: name (and the working timer) on top, the latest
 * message below in up to two lines. A right-hand column keeps the time on
 * top and the row's badges under it, so the text never runs under them.
 */
export function ChatRowLines({
  badges,
  bold,
  dimmed,
  preview,
  title,
  titleExtras,
  working,
}: {
  badges?: React.ReactNode;
  bold: boolean;
  dimmed: boolean;
  preview: ChatRowPreview;
  title: React.ReactNode;
  /** Small marks after the name (pin, mute). */
  titleExtras?: React.ReactNode;
  /** The working timer, at the end of the name line, left of the time. */
  working?: React.ReactNode;
}) {
  return (
    <span
      className={cn(
        "flex min-w-0 flex-1 items-start gap-2",
        dimmed && "sidebar-muted-content opacity-50 dark:opacity-45",
      )}
      data-sidebar-row-label
    >
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="flex min-w-0 items-center gap-1">
          <span
            className={cn(
              "flex min-w-0 items-center gap-1 truncate",
              bold && "font-bold",
            )}
          >
            {title}
          </span>
          {titleExtras}
          {working ? (
            <span className="ml-auto flex shrink-0 items-center">
              {working}
            </span>
          ) : null}
        </span>
        {preview.line ? (
          <span
            className={cn(
              "line-clamp-2 whitespace-normal break-words text-xs font-normal leading-snug",
              bold
                ? "text-sidebar-foreground/85"
                : "text-sidebar-foreground/55",
            )}
          >
            {preview.line}
          </span>
        ) : null}
      </span>
      <span className="flex shrink-0 flex-col items-end gap-1 pt-px">
        <span className="text-2xs font-normal leading-5 text-sidebar-foreground/50 tabular-nums">
          {preview.at !== null ? formatItemTimestamp(preview.at) : ""}
        </span>
        {badges ? (
          <span className="flex items-center gap-1">{badges}</span>
        ) : null}
      </span>
    </span>
  );
}
