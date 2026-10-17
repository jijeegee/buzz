import * as React from "react";
import { MessageSquare, Star, StarOff } from "lucide-react";

import { useManagedAgentsQuery } from "@/features/agents/hooks";
import { useIsArchivedPredicate } from "@/features/identity-archive/hooks";
import { usePeopleFavorites } from "@/features/people/PeopleFavoritesContext";
import {
  buildPeopleDirectory,
  personName,
} from "@/features/people/lib/peopleDirectory";
import {
  useFlattenedUserSearchResults,
  useInfiniteUserSearchQuery,
  useUsersBatchQuery,
} from "@/features/profile/hooks";
import { AgentBadgeIcon } from "@/features/profile/ui/AgentBadgeIcon";
import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import {
  ContextMenuIconSlot,
  deferMenuAction,
} from "@/features/sidebar/ui/sidebarMenuHelpers";
import type { UserSearchResult } from "@/shared/api/types";
import { cn } from "@/shared/lib/cn";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/shared/ui/context-menu";
import { Input } from "@/shared/ui/input";
import { PageHeader, SectionHeader } from "@/shared/ui/PageHeader";
import { Skeleton } from "@/shared/ui/skeleton";

const DIRECTORY_PAGE_SIZE = 500;

type PeopleViewProps = {
  currentPubkey: string | undefined;
  onMessage: (pubkey: string) => void;
  onOpenProfile: (pubkey: string) => void;
  selectedPubkey: string | null;
};

/** Favorites on top, then everyone in the community A–Z. */
export function PeopleView({
  currentPubkey,
  onMessage,
  onOpenProfile,
  selectedPubkey,
}: PeopleViewProps) {
  const [filter, setFilter] = React.useState("");
  const favorites = usePeopleFavorites();
  const usersQuery = useInfiniteUserSearchQuery("", {
    allowEmpty: true,
    limit: DIRECTORY_PAGE_SIZE,
  });
  const { fetchNextPage, hasNextPage, isFetchingNextPage } = usersQuery;
  // The directory is filtered and sorted on this device, so load every page.
  React.useEffect(() => {
    if (hasNextPage && !isFetchingNextPage) void fetchNextPage();
  }, [fetchNextPage, hasNextPage, isFetchingNextPage]);
  const users = useFlattenedUserSearchResults(usersQuery.data);
  const managedAgentsQuery = useManagedAgentsQuery();
  const myAgentPubkeys = React.useMemo(
    () =>
      new Set(
        (managedAgentsQuery.data ?? []).map((agent) =>
          agent.pubkey.toLowerCase(),
        ),
      ),
    [managedAgentsQuery.data],
  );
  const isArchived = useIsArchivedPredicate();
  const favoriteIds = favorites?.favoriteIds ?? EMPTY_IDS;
  const favoriteProfilesQuery = useUsersBatchQuery([...favoriteIds], {
    enabled: favoriteIds.size > 0,
  });
  const directory = React.useMemo(
    () =>
      buildPeopleDirectory({
        currentPubkey,
        favoriteIds,
        favoriteProfiles: favoriteProfilesQuery.data?.profiles,
        filter,
        isArchived,
        myAgentPubkeys,
        users,
      }),
    [
      currentPubkey,
      favoriteIds,
      favoriteProfilesQuery.data?.profiles,
      filter,
      isArchived,
      myAgentPubkeys,
      users,
    ],
  );

  const renderRow = (user: UserSearchResult, section: string) => (
    <PersonRow
      isFavorite={favorites?.isFavorite(user.pubkey) ?? false}
      isSelected={selectedPubkey?.toLowerCase() === user.pubkey}
      key={`${section}:${user.pubkey}`}
      onMessage={onMessage}
      onOpenProfile={onOpenProfile}
      onToggleFavorite={
        favorites
          ? (pubkey, next) =>
              next
                ? favorites.addFavorite(pubkey)
                : favorites.removeFavorite(pubkey)
          : undefined
      }
      user={user}
    />
  );

  return (
    <div className="flex-1 overflow-y-auto overflow-x-hidden overscroll-contain px-4 py-7 sm:px-6 sm:py-8">
      <div
        className="mx-auto w-full max-w-3xl space-y-6"
        data-testid="people-page-content"
      >
        <PageHeader title="People" />
        <Input
          aria-label="Filter people by name"
          data-testid="people-filter"
          onChange={(event) => setFilter(event.target.value)}
          placeholder="Filter by name"
          value={filter}
        />
        {directory.favorites.length > 0 ? (
          <section className="space-y-2" data-testid="people-favorites">
            <SectionHeader title="Favorites" />
            <div className="flex flex-col">
              {directory.favorites.map((user) => renderRow(user, "favorite"))}
            </div>
          </section>
        ) : null}
        <section className="space-y-2" data-testid="people-everyone">
          <SectionHeader title="Everyone" />
          {usersQuery.isLoading ? (
            <PeopleSkeleton />
          ) : directory.everyone.length === 0 ? (
            <p className="px-2 py-6 text-sm text-muted-foreground">
              {filter.trim() ? "No one matches that name." : "No one here yet."}
            </p>
          ) : (
            <div className="flex flex-col">
              {directory.everyone.map((user) => renderRow(user, "everyone"))}
            </div>
          )}
        </section>
      </div>
    </div>
  );
}

const EMPTY_IDS: ReadonlySet<string> = new Set();

function PersonRow({
  isFavorite,
  isSelected,
  onMessage,
  onOpenProfile,
  onToggleFavorite,
  user,
}: {
  isFavorite: boolean;
  isSelected: boolean;
  onMessage: (pubkey: string) => void;
  onOpenProfile: (pubkey: string) => void;
  onToggleFavorite?: (pubkey: string, favorite: boolean) => void;
  user: UserSearchResult;
}) {
  const name = personName(user);
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <button
          aria-current={isSelected || undefined}
          className={cn(
            "flex min-h-12 w-full items-center gap-3 rounded-lg px-2 py-2 text-left transition-colors hover:bg-muted/40 focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring",
            isSelected && "bg-muted/60",
          )}
          data-testid={`people-row-${user.pubkey}`}
          onClick={() => onOpenProfile(user.pubkey)}
          type="button"
        >
          <ProfileAvatar
            avatarUrl={user.avatarUrl}
            className="h-8 w-8 text-xs shadow-none"
            iconClassName="h-4 w-4"
            label={name}
            shape={user.isAgent ? "squircle" : "circle"}
          />
          <span className="min-w-0 flex-1 truncate text-sm font-medium">
            {name}
          </span>
          {user.isAgent ? (
            <span
              className="inline-flex shrink-0 items-center gap-1 text-xs text-muted-foreground"
              data-testid="people-agent-badge"
            >
              <AgentBadgeIcon
                agentPubkey={user.pubkey}
                className="h-3 w-3"
                ownerPubkey={user.ownerPubkey}
              />
              agent
            </span>
          ) : null}
          {isFavorite ? (
            <Star
              aria-label="Favorite"
              className="h-3.5 w-3.5 shrink-0 text-muted-foreground"
            />
          ) : null}
        </button>
      </ContextMenuTrigger>
      <ContextMenuContent>
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
            {isFavorite ? "Remove from favorites" : "Add to favorites"}
          </ContextMenuItem>
        ) : null}
        <ContextMenuItem
          onSelect={() => deferMenuAction(() => onMessage(user.pubkey))}
        >
          <ContextMenuIconSlot>
            <MessageSquare className="h-4 w-4" />
          </ContextMenuIconSlot>
          Message
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}

function PeopleSkeleton() {
  return (
    <div className="space-y-3 px-2 py-2">
      {[1, 2, 3, 4].map((i) => (
        <div className="flex items-center gap-3" key={i}>
          <Skeleton className="h-8 w-8 shrink-0 rounded-full" />
          <Skeleton className="h-4 w-40" />
        </div>
      ))}
    </div>
  );
}
