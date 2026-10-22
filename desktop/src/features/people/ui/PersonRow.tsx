import {
  MessageSquare,
  Star,
  StarOff,
  UserMinus,
  UserPlus,
} from "lucide-react";

import { personName } from "@/features/people/lib/peopleDirectory";
import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import {
  ContextMenuIconSlot,
  deferMenuAction,
} from "@/features/sidebar/ui/sidebarMenuHelpers";
import type { UserSearchResult } from "@/shared/api/types";
import { useT } from "@/shared/i18n";
import { cn } from "@/shared/lib/cn";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/shared/ui/context-menu";

/** One person in the People list; right-click to star, add or message. */
export function PersonRow({
  isFavorite,
  isFriend,
  isSelected,
  onMessage,
  onOpenProfile,
  onToggleFavorite,
  onToggleFriend,
  user,
}: {
  isFavorite: boolean;
  isFriend: boolean;
  isSelected: boolean;
  onMessage: (pubkey: string) => void;
  onOpenProfile: (pubkey: string) => void;
  onToggleFavorite?: (pubkey: string, favorite: boolean) => void;
  onToggleFriend: (pubkey: string, friend: boolean) => void;
  user: UserSearchResult;
}) {
  const t = useT();
  const name = personName(user);
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          className={cn(
            "flex h-12 w-full items-center rounded-md text-sm text-sidebar-foreground transition-colors hover:bg-sidebar-accent",
            isSelected && "bg-sidebar-accent",
          )}
        >
          <button
            aria-current={isSelected || undefined}
            className="flex h-full min-w-0 flex-1 items-center gap-2.5 rounded-md px-2 text-left outline-hidden focus-visible:ring-2 focus-visible:ring-sidebar-ring"
            data-testid={`people-row-${user.pubkey}`}
            onClick={() => onOpenProfile(user.pubkey)}
            type="button"
          >
            <ProfileAvatar
              mediaVariant="small"
              avatarUrl={user.avatarUrl}
              className="h-9 w-9 text-xs shadow-none"
              iconClassName="h-4 w-4"
              label={name}
              shape="circle"
            />
            <span className="min-w-0 flex-1 truncate font-medium">{name}</span>
            {isFavorite ? (
              <Star
                aria-label={t("people.row.favorite")}
                className="h-3.5 w-3.5 shrink-0 fill-amber-400 text-amber-400"
              />
            ) : null}
          </button>
          {!isFriend ? (
            <button
              aria-label={t("people.row.addAsFriend", { name })}
              className="mr-1.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-sidebar-foreground/70 outline-hidden hover:bg-sidebar-foreground/10 hover:text-sidebar-foreground focus-visible:ring-2 focus-visible:ring-sidebar-ring"
              data-testid={`people-add-friend-${user.pubkey}`}
              onClick={() => onToggleFriend(user.pubkey, true)}
              title={t("people.friend.add")}
              type="button"
            >
              <UserPlus className="h-4 w-4" />
            </button>
          ) : null}
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent>
        <ContextMenuItem
          onSelect={() =>
            deferMenuAction(() => onToggleFriend(user.pubkey, !isFriend))
          }
        >
          <ContextMenuIconSlot>
            {isFriend ? (
              <UserMinus className="h-4 w-4" />
            ) : (
              <UserPlus className="h-4 w-4" />
            )}
          </ContextMenuIconSlot>
          {isFriend ? t("people.friend.remove") : t("people.friend.add")}
        </ContextMenuItem>
        {onToggleFavorite ? (
          <ContextMenuItem
            onSelect={() =>
              deferMenuAction(() => onToggleFavorite(user.pubkey, !isFavorite))
            }
          >
            <ContextMenuIconSlot>
              {isFavorite ? (
                <StarOff className="h-4 w-4" />
              ) : (
                <Star className="h-4 w-4" />
              )}
            </ContextMenuIconSlot>
            {isFavorite
              ? t("people.favorite.remove")
              : t("people.favorite.add")}
          </ContextMenuItem>
        ) : null}
        <ContextMenuItem
          onSelect={() => deferMenuAction(() => onMessage(user.pubkey))}
        >
          <ContextMenuIconSlot>
            <MessageSquare className="h-4 w-4" />
          </ContextMenuIconSlot>
          {t("people.message")}
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}
