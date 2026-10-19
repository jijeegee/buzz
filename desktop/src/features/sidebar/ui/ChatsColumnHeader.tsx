import type * as React from "react";
import { MessageSquarePlus, Plus, UsersRound } from "lucide-react";

import type { ChatsViewMode } from "@/features/chats/chatListGroups";
import { deferMenuAction } from "@/features/sidebar/ui/sidebarMenuHelpers";
import { cn } from "@/shared/lib/cn";
import { Button } from "@/shared/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/shared/ui/dropdown-menu";

/**
 * "Chats" with Find groups, + New (New message / New group / Find groups)
 * and the list menu, atop the list column.
 */
export function ChatsColumnTitle({
  menu,
  onBrowseChannels,
  onNewGroup,
  onNewMessage,
}: {
  menu?: React.ReactNode;
  onBrowseChannels?: () => void;
  onNewGroup?: () => void;
  onNewMessage: () => void;
}) {
  const hasNewMenu = Boolean(onNewGroup || onBrowseChannels);
  const newButton = (
    <Button
      className="h-7 gap-1 px-2 text-xs"
      data-testid="section-actions-chats-quick-create"
      onClick={hasNewMenu ? undefined : onNewMessage}
      size="sm"
    >
      <Plus className="h-3.5 w-3.5" />
      New
    </Button>
  );
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
          <UsersRound className="h-3.5 w-3.5" />
          Find groups
        </Button>
      ) : null}
      {hasNewMenu ? (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>{newButton}</DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-44">
            <DropdownMenuItem
              data-testid="chats-new-menu-message"
              onSelect={() => deferMenuAction(onNewMessage)}
            >
              <MessageSquarePlus className="h-4 w-4" />
              <span>New message</span>
            </DropdownMenuItem>
            {onNewGroup ? (
              <DropdownMenuItem
                data-testid="chats-new-menu-group"
                onSelect={() => deferMenuAction(onNewGroup)}
              >
                <Plus className="h-4 w-4" />
                <span>New group</span>
              </DropdownMenuItem>
            ) : null}
            {onBrowseChannels ? (
              <DropdownMenuItem
                data-testid="chats-new-menu-find-groups"
                onSelect={() => deferMenuAction(onBrowseChannels)}
              >
                <UsersRound className="h-4 w-4" />
                <span>Find groups</span>
              </DropdownMenuItem>
            ) : null}
          </DropdownMenuContent>
        </DropdownMenu>
      ) : (
        newButton
      )}
      {menu}
    </div>
  );
}

const VIEWS: { label: string; testId: string; value: ChatsViewMode }[] = [
  { label: "All chats", testId: "chats-toggle-all", value: "all" },
  {
    label: "By group",
    testId: "chats-toggle-by-channel",
    value: "by-channel",
  },
];

/** All chats / By group, as two tabs under the search box. */
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
