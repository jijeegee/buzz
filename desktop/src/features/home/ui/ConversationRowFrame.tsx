import * as React from "react";

import { cn } from "@/shared/lib/cn";

const ROW_HIGHLIGHT_BG =
  "color-mix(in srgb, hsl(var(--background)) 75%, hsl(var(--muted)) 25%)";
const ROW_SELECTED_BG =
  "color-mix(in srgb, hsl(var(--background)) 70%, hsl(var(--muted)) 30%)";

type ConversationRowFrameProps = Omit<
  React.HTMLAttributes<HTMLDivElement>,
  "title" | "children" | "onSelect"
> & {
  /** Accessible name of the full-row button. */
  ariaLabel: string;
  avatar: React.ReactNode;
  /** Props for the full-row button (test ids, data attributes). */
  buttonProps?: React.ButtonHTMLAttributes<HTMLButtonElement> &
    Record<`data-${string}`, string | undefined>;
  /** Lines under the title: labels, status, the message preview. */
  children?: React.ReactNode;
  /** Extra classes for the padded content box (e.g. an indent). */
  contentClassName?: string;
  /** Hides `meta` on hover, for rows whose hover actions sit over it. */
  hideMetaOnHover?: boolean;
  /** Buttons shown over the top-right corner on hover. */
  hoverActions?: React.ReactNode;
  isSelected: boolean;
  /** Right end of the title line: time, counts, status. */
  meta?: React.ReactNode;
  metaClassName?: string;
  /** Clicks on the row content; defaults to `onSelect`. */
  onContentClick?: (event: React.MouseEvent<HTMLElement>) => void;
  onSelect: () => void;
  title: React.ReactNode;
};

/**
 * The look of a conversation row, shared by the Inbox and Chats lists:
 * edge to edge, avatar on the left, title and meta on the first line, and a
 * soft tint on hover and when selected. Data and loading stay with each list.
 */
export const ConversationRowFrame = React.forwardRef<
  HTMLDivElement,
  ConversationRowFrameProps
>(function ConversationRowFrame(
  {
    ariaLabel,
    avatar,
    buttonProps,
    children,
    className,
    contentClassName,
    hideMetaOnHover = false,
    hoverActions,
    isSelected,
    meta,
    metaClassName,
    onContentClick,
    onSelect,
    style,
    title,
    ...props
  },
  ref,
) {
  return (
    <div
      {...props}
      aria-current={isSelected ? "true" : undefined}
      className={cn("group/inbox-item relative", className)}
      data-conversation-row=""
      ref={ref}
      style={
        {
          ...style,
          "--inbox-row-highlight-bg": isSelected
            ? ROW_SELECTED_BG
            : ROW_HIGHLIGHT_BG,
        } as React.CSSProperties
      }
    >
      <button
        {...buttonProps}
        aria-label={ariaLabel}
        className="absolute inset-0 z-0 block w-full border-l border-l-transparent text-left"
        onClick={onSelect}
        type="button"
      >
        <span
          aria-hidden="true"
          className={cn(
            "pointer-events-none absolute inset-y-0 left-0 right-0 transition-colors",
            isSelected
              ? "bg-[var(--inbox-row-highlight-bg)]"
              : "group-hover/inbox-item:bg-[var(--inbox-row-highlight-bg)] group-focus-within/inbox-item:bg-[var(--inbox-row-highlight-bg)] group-active/inbox-item:bg-muted/40",
          )}
        />
      </button>

      {/* biome-ignore lint/a11y: The sibling full-row button provides keyboard/screen-reader row activation; this wrapper delegates pointer selection while allowing nested controls. */}
      <div
        className={cn(
          "relative z-10 block w-full cursor-pointer px-3 py-4 text-left",
          contentClassName,
        )}
        onClick={onContentClick ?? onSelect}
      >
        <div className="flex min-w-0 items-start gap-2.5">
          {avatar}
          <div className="min-w-0 flex-1">
            <div className="flex min-w-0 items-start gap-2">
              {title}
              {meta ? (
                <span
                  className={cn(
                    "flex shrink-0 items-center gap-1.5 text-xs leading-4 text-muted-foreground/70",
                    hideMetaOnHover &&
                      "transition-opacity group-hover/inbox-item:opacity-0 group-focus-within/inbox-item:opacity-0",
                    metaClassName,
                  )}
                >
                  {meta}
                </span>
              ) : null}
            </div>
            {children}
          </div>
        </div>
      </div>

      {hoverActions ? (
        <div className="pointer-events-none absolute right-3 top-2 z-10 flex items-center gap-0.5 rounded-full bg-[var(--inbox-row-highlight-bg)] p-1 opacity-0 transition-opacity duration-150 ease-out group-hover/inbox-item:pointer-events-auto group-hover/inbox-item:opacity-100 group-focus-within/inbox-item:pointer-events-auto group-focus-within/inbox-item:opacity-100">
          {hoverActions}
        </div>
      ) : null}
    </div>
  );
});

/** The title text of a conversation row. */
export const CONVERSATION_ROW_TITLE_CLASS =
  "block min-w-0 flex-1 truncate text-sm font-semibold leading-4 text-foreground";

/** The message preview under the title; bold while it is unread. */
export function conversationRowPreviewClass(unread: boolean): string {
  return cn(
    "mt-1.5 text-message [&_a]:font-medium [&_a]:text-current",
    unread
      ? "font-semibold text-foreground"
      : "font-normal text-muted-foreground",
  );
}
