import * as React from "react";
import { useLocation, useNavigate } from "@tanstack/react-router";
import { toast } from "sonner";

import { useIsArchivedPredicate } from "@/features/identity-archive/hooks";
import { usePeopleFavorites } from "@/features/people/PeopleFavoritesContext";
import { buildPeopleDirectory } from "@/features/people/lib/peopleDirectory";
import { useMessagePerson } from "@/features/people/useMessagePerson";
import {
  usePeopleFriends,
  useSeedFriendsFromDms,
} from "@/features/people/usePeopleFriends";
import {
  useFlattenedUserSearchResults,
  useInfiniteUserSearchQuery,
  useUsersBatchQuery,
} from "@/features/profile/hooks";
import type { UserSearchResult } from "@/shared/api/types";
import { useHistorySearchState } from "@/shared/hooks/useHistorySearchState";
import { Input } from "@/shared/ui/input";
import { Skeleton } from "@/shared/ui/skeleton";
import { PersonRow } from "./PersonRow";

const DIRECTORY_PAGE_SIZE = 500;
const PROFILE_KEYS = ["profile", "profileTab", "profileView"] as const;
const EMPTY_IDS: ReadonlySet<string> = new Set();

/**
 * The People tab's list, in the left column: Favorites, then Friends (people
 * I added). Typing a name also finds others so they can be added. People
 * only; agents live in the Agents tab. Picking someone shows their profile on
 * the right.
 */
export function PeopleSidebarList({
  currentPubkey,
}: {
  currentPubkey: string | undefined;
}) {
  const [filter, setFilter] = React.useState("");
  const favorites = usePeopleFavorites();
  const friends = usePeopleFriends(currentPubkey);
  useSeedFriendsFromDms(currentPubkey);
  const onMessage = useMessagePerson();
  const navigate = useNavigate();
  const pathname = useLocation({ select: (location) => location.pathname });
  const { values } = useHistorySearchState(PROFILE_KEYS);
  const selected =
    pathname === "/people" ? (values.profile?.toLowerCase() ?? null) : null;
  // Opening a person is what changes the main pane, not the tab.
  const onOpenProfile = React.useCallback(
    (pubkey: string) =>
      void navigate({ to: "/people", search: { profile: pubkey } }),
    [navigate],
  );

  const usersQuery = useInfiniteUserSearchQuery("", {
    allowEmpty: true,
    limit: DIRECTORY_PAGE_SIZE,
  });
  const { fetchNextPage, hasNextPage, isFetchingNextPage } = usersQuery;
  // Filtered and sorted on this device, so load every page.
  React.useEffect(() => {
    if (hasNextPage && !isFetchingNextPage) void fetchNextPage();
  }, [fetchNextPage, hasNextPage, isFetchingNextPage]);
  const users = useFlattenedUserSearchResults(usersQuery.data);
  const isArchived = useIsArchivedPredicate();
  const favoriteIds = favorites?.favoriteIds ?? EMPTY_IDS;
  const listedIds = React.useMemo(
    () => [...new Set([...favoriteIds, ...friends.friendIds])],
    [favoriteIds, friends.friendIds],
  );
  const extraProfilesQuery = useUsersBatchQuery(listedIds, {
    enabled: listedIds.length > 0,
  });
  const directory = React.useMemo(
    () =>
      buildPeopleDirectory({
        currentPubkey,
        extraProfiles: extraProfilesQuery.data?.profiles,
        favoriteIds,
        filter,
        friendIds: friends.friendIds,
        isArchived,
        users,
      }),
    [
      currentPubkey,
      extraProfilesQuery.data?.profiles,
      favoriteIds,
      filter,
      friends.friendIds,
      isArchived,
      users,
    ],
  );

  const onToggleFriend = (pubkey: string, next: boolean) => {
    (next ? friends.addFriend(pubkey) : friends.removeFriend(pubkey)).catch(
      (error: unknown) =>
        toast.error(
          error instanceof Error
            ? error.message
            : next
              ? "Could not add friend"
              : "Could not remove friend",
        ),
    );
  };
  const renderRow = (user: UserSearchResult, section: string) => (
    <PersonRow
      isFavorite={favorites?.isFavorite(user.pubkey) ?? false}
      isFriend={friends.isFriend(user.pubkey)}
      isSelected={selected === user.pubkey}
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
      onToggleFriend={onToggleFriend}
      user={user}
    />
  );
  const searching = filter.trim().length > 0;
  const loading = friends.isLoading || usersQuery.isLoading;

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="people-list">
      <div className="space-y-2 px-2 pb-2 pt-1">
        <h2 className="px-1 text-base font-semibold text-sidebar-foreground">
          People
        </h2>
        <Input
          aria-label="Find people by name"
          className="h-8 bg-background"
          data-testid="people-filter"
          onChange={(event) => setFilter(event.target.value)}
          placeholder="Find people"
          value={filter}
        />
      </div>
      <div className="min-h-0 flex-1 space-y-3 overflow-y-auto overflow-x-hidden px-1 pb-4">
        {directory.favorites.length > 0 ? (
          <Section testId="people-favorites" title="Favorites">
            {directory.favorites.map((user) => renderRow(user, "favorite"))}
          </Section>
        ) : null}
        <Section
          testId="people-friends"
          title={`Friends · ${directory.friends.length}`}
        >
          {loading ? (
            <PeopleSkeleton />
          ) : directory.friends.length > 0 ? (
            directory.friends.map((user) => renderRow(user, "friend"))
          ) : (
            <p className="px-2 py-3 text-xs text-sidebar-foreground/60">
              {searching
                ? "No friends match that name."
                : "No friends yet. Find someone by name above and add them."}
            </p>
          )}
        </Section>
        {searching ? (
          <Section testId="people-others" title="Others">
            {directory.others.length > 0 ? (
              directory.others.map((user) => renderRow(user, "other"))
            ) : (
              <p className="px-2 py-3 text-xs text-sidebar-foreground/60">
                No one else matches that name.
              </p>
            )}
          </Section>
        ) : null}
      </div>
    </div>
  );
}

function Section({
  children,
  testId,
  title,
}: {
  children: React.ReactNode;
  testId: string;
  title: string;
}) {
  return (
    <section data-testid={testId}>
      <h3 className="px-2 pb-1 text-2xs font-semibold uppercase tracking-wide text-sidebar-foreground/55">
        {title}
      </h3>
      <div className="flex flex-col">{children}</div>
    </section>
  );
}

function PeopleSkeleton() {
  return (
    <div className="space-y-3 px-2 py-2">
      {[1, 2, 3].map((i) => (
        <div className="flex items-center gap-2.5" key={i}>
          <Skeleton className="h-9 w-9 shrink-0 rounded-full" />
          <Skeleton className="h-4 w-28" />
        </div>
      ))}
    </div>
  );
}
