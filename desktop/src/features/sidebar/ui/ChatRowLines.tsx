import type * as React from "react";

import type { ChatRowPreview } from "@/features/chats/chatRowPreview";
import { cn } from "@/shared/lib/cn";
import { formatItemTimestamp } from "@/shared/lib/datetime";

/**
 * The text of a two-line Chats row: name and time on top, the latest
 * message and the row's badges below.
 */
export function ChatRowLines({
  badges,
  bold,
  dimmed,
  preview,
  title,
  titleExtras,
}: {
  badges?: React.ReactNode;
  bold: boolean;
  dimmed: boolean;
  preview: ChatRowPreview;
  title: React.ReactNode;
  /** Small marks after the name (pin, mute, working). */
  titleExtras?: React.ReactNode;
}) {
  return (
    <span
      className={cn(
        "flex min-w-0 flex-1 flex-col gap-0.5",
        dimmed && "sidebar-muted-content opacity-50 dark:opacity-45",
      )}
      data-sidebar-row-label
    >
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
        {preview.at !== null ? (
          <span className="ml-auto shrink-0 pl-1 text-2xs font-normal text-sidebar-foreground/50 tabular-nums">
            {formatItemTimestamp(preview.at)}
          </span>
        ) : null}
      </span>
      <span className="flex min-w-0 items-center gap-1">
        <span
          className={cn(
            "min-w-0 flex-1 truncate text-xs font-normal",
            bold ? "text-sidebar-foreground/85" : "text-sidebar-foreground/55",
          )}
        >
          {preview.line ?? ""}
        </span>
        {badges}
      </span>
    </span>
  );
}
