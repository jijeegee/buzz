import type { UserProfileSummary, UserSearchResult } from "@/shared/api/types";

export type PeopleDirectory = {
  favorites: UserSearchResult[];
  everyone: UserSearchResult[];
};

const UNKNOWN_PERSON = "Unknown";

export function personName(user: UserSearchResult): string {
  return user.displayName?.trim() || user.nip05Handle?.trim() || UNKNOWN_PERSON;
}

const byName = (left: UserSearchResult, right: UserSearchResult) =>
  personName(left).localeCompare(personName(right), undefined, {
    sensitivity: "base",
  }) || (left.pubkey < right.pubkey ? -1 : left.pubkey > right.pubkey ? 1 : 0);

/**
 * The People screen lists: Favorites (people I starred) and Everyone in the
 * community, both A–Z and narrowed by the name filter. Me and my own agents
 * are left out (Agents has its own tab); other people's agents stay and are
 * shown with a bot badge. A favorite not in the loaded directory still shows
 * from its batch-fetched profile.
 */
export function buildPeopleDirectory({
  currentPubkey,
  favoriteIds,
  favoriteProfiles,
  filter,
  isArchived,
  myAgentPubkeys,
  users,
}: {
  currentPubkey: string | undefined;
  favoriteIds: ReadonlySet<string>;
  favoriteProfiles?: Readonly<Record<string, UserProfileSummary>>;
  filter: string;
  isArchived?: (pubkey: string) => boolean;
  myAgentPubkeys: ReadonlySet<string>;
  users: readonly UserSearchResult[];
}): PeopleDirectory {
  const self = currentPubkey?.toLowerCase() ?? null;
  const excluded = (user: UserSearchResult) => {
    const pubkey = user.pubkey.toLowerCase();
    return (
      pubkey === self ||
      myAgentPubkeys.has(pubkey) ||
      (self !== null && user.ownerPubkey?.toLowerCase() === self) ||
      (isArchived?.(pubkey) ?? false)
    );
  };
  const needle = filter.trim().toLowerCase();
  const matches = (user: UserSearchResult) =>
    needle.length === 0 ||
    personName(user).toLowerCase().includes(needle) ||
    (user.nip05Handle?.toLowerCase().includes(needle) ?? false);

  const byPubkey = new Map<string, UserSearchResult>();
  for (const user of users) {
    const pubkey = user.pubkey.toLowerCase();
    if (!byPubkey.has(pubkey)) byPubkey.set(pubkey, { ...user, pubkey });
  }
  const everyone = [...byPubkey.values()].filter(
    (user) => !excluded(user) && matches(user),
  );

  const favorites: UserSearchResult[] = [];
  for (const id of favoriteIds) {
    const pubkey = id.toLowerCase();
    const profile = favoriteProfiles?.[pubkey];
    const user =
      byPubkey.get(pubkey) ??
      (profile
        ? {
            pubkey,
            displayName: profile.displayName,
            avatarUrl: profile.avatarUrl,
            nip05Handle: profile.nip05Handle,
            ownerPubkey: profile.ownerPubkey,
            isAgent: profile.isAgent ?? false,
          }
        : null);
    if (user && !excluded(user) && matches(user)) favorites.push(user);
  }

  return {
    favorites: favorites.sort(byName),
    everyone: everyone.sort(byName),
  };
}
