import type { UserProfileSummary, UserSearchResult } from "@/shared/api/types";

export type PeopleDirectory = {
  favorites: UserSearchResult[];
  /** People I added as friends (my follow list), A–Z. */
  friends: UserSearchResult[];
  /** Everyone else matching the filter; empty while the filter is empty. */
  others: UserSearchResult[];
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
 * The People list: Favorites (people I starred) and Friends (people I added),
 * both A–Z and narrowed by the name filter. Typing a name also finds others
 * in the community so they can be added. People only: me, archived people
 * and every agent are left out (agents have their own tab). A favorite or
 * friend not in the loaded directory still shows from its batch profile.
 */
export function buildPeopleDirectory({
  currentPubkey,
  favoriteIds,
  friendIds,
  extraProfiles,
  filter,
  isArchived,
  users,
}: {
  currentPubkey: string | undefined;
  favoriteIds: ReadonlySet<string>;
  friendIds: ReadonlySet<string>;
  extraProfiles?: Readonly<Record<string, UserProfileSummary>>;
  filter: string;
  isArchived?: (pubkey: string) => boolean;
  users: readonly UserSearchResult[];
}): PeopleDirectory {
  const self = currentPubkey?.toLowerCase() ?? null;
  const excluded = (user: UserSearchResult) => {
    const pubkey = user.pubkey.toLowerCase();
    return pubkey === self || user.isAgent || (isArchived?.(pubkey) ?? false);
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
  const listed = (ids: ReadonlySet<string>) => {
    const list: UserSearchResult[] = [];
    for (const id of ids) {
      const pubkey = id.toLowerCase();
      const profile = extraProfiles?.[pubkey];
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
      if (user && !excluded(user) && matches(user)) list.push(user);
    }
    return list.sort(byName);
  };
  const friendSet = new Set([...friendIds].map((id) => id.toLowerCase()));
  const others =
    needle.length === 0
      ? []
      : [...byPubkey.values()]
          .filter(
            (user) =>
              !friendSet.has(user.pubkey) && !excluded(user) && matches(user),
          )
          .sort(byName);

  return {
    favorites: listed(favoriteIds),
    friends: listed(friendSet),
    others,
  };
}
