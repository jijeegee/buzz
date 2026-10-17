import type * as React from "react";
import { Hash, Plus } from "lucide-react";

import type { ChatsViewMode } from "@/features/chats/chatListGroups";
import { cn } from "@/shared/lib/cn";
import { Button } from "@/shared/ui/button";

/** "Chats" with # Channels, + New and the list menu, atop the list column. */
export function ChatsColumnTitle({
  menu,
  onBrowseChannels,
  onNewMessage,
}: {
  menu?: React.ReactNode;
  onBrowseChannels?: () => void;
  onNewMessage: () => void;
}) {
  return (
    <div
      className="flex items-center gap-1.5 px-3 pb-1 pt-1"
      data-testid="chats-column-title"
    >
      <h2 className="min-w-0 flex-1 truncate text-base font-semibold text-sidebar-foreground">
        Chats
      </h2>
      {onBrowseChannels ? (
        <Button
          className="h-7 gap-1 px-2 text-xs"
          data-testid="chats-browse-channels"
          onClick={onBrowseChannels}
          size="sm"
          variant="outline"
        >
          <Hash className="h-3.5 w-3.5" />
          Channels
        </Button>
      ) : null}
      <Button
        className="h-7 gap-1 px-2 text-xs"
        data-testid="section-actions-chats-quick-create"
        onClick={onNewMessage}
        size="sm"
      >
        <Plus className="h-3.5 w-3.5" />
        New
      </Button>
      {menu}
    </div>
  );
}

const VIEWS: { label: string; testId: string; value: ChatsViewMode }[] = [
  { label: "All chats", testId: "chats-toggle-all", value: "all" },
  {
    label: "By channel",
    testId: "chats-toggle-by-channel",
    value: "by-channel",
  },
];

/** All chats / By channel, as two tabs under the search box. */
export function ChatsViewToggle({
  onChange,
  value,
}: {
  onChange: (value: ChatsViewMode) => void;
  value: ChatsViewMode;
}) {
  return (
    <div
      aria-label="Chats view"
      className="mx-3 mb-1 flex rounded-lg bg-sidebar-accent p-0.5"
      role="tablist"
    >
      {VIEWS.map((view) => (
        <button
          aria-selected={value === view.value}
          className={cn(
            "flex-1 rounded-md px-2 py-1 text-xs font-medium text-sidebar-foreground/65 outline-hidden transition-colors hover:text-sidebar-foreground focus-visible:ring-2 focus-visible:ring-sidebar-ring",
            value === view.value &&
              "bg-background text-sidebar-foreground shadow-xs",
          )}
          data-testid={view.testId}
          key={view.value}
          onClick={() => onChange(view.value)}
          role="tab"
          type="button"
        >
          {view.label}
        </button>
      ))}
    </div>
  );
}
