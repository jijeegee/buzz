import type { UserProfileSummary, UserSearchResult } from "@/shared/api/types";
import { t } from "@/shared/i18n";

export type PeopleDirectory = {
  favorites: UserSearchResult[];
  /** People I added as friends (my follow list), A–Z. */
  friends: UserSearchResult[];
  /**
   * People in my channels whose name matches the search (from the server),
   * minus my friends; empty while the filter is empty.
   */
  inChannels: UserSearchResult[];
};

export function personName(user: UserSearchResult): string {
  return (
    user.displayName?.trim() ||
    user.nip05Handle?.trim() ||
    t("people.unknownPerson")
  );
}

const byName = (left: UserSearchResult, right: UserSearchResult) =>
  personName(left).localeCompare(personName(right), undefined, {
    sensitivity: "base",
  }) || (left.pubkey < right.pubkey ? -1 : left.pubkey > right.pubkey ? 1 : 0);

/**
 * The People list: Favorites (people I starred) and Friends (people I added),
 * both A–Z and narrowed by the name filter, which searches only them. While
 * searching, "In your channels" adds people I share a channel with (the
 * server's `sharedPeople`) so they can be added. People only: me, archived
 * and blocked people and every agent are left out (agents have their own
 * tab). Favorites and friends show from their batch profiles (`extraProfiles`)
 * or any other known profile (`users`).
 */
export function buildPeopleDirectory({
  currentPubkey,
  favoriteIds,
  friendIds,
  extraProfiles,
  filter,
  isArchived,
  isBlocked,
  sharedPeople = [],
  users = [],
}: {
  currentPubkey: string | undefined;
  favoriteIds: ReadonlySet<string>;
  friendIds: ReadonlySet<string>;
  extraProfiles?: Readonly<Record<string, UserProfileSummary>>;
  filter: string;
  isArchived?: (pubkey: string) => boolean;
  isBlocked?: (pubkey: string) => boolean;
  sharedPeople?: readonly UserSearchResult[];
  users?: readonly UserSearchResult[];
}): PeopleDirectory {
  const self = currentPubkey?.toLowerCase() ?? null;
  const excluded = (user: UserSearchResult) => {
    const pubkey = user.pubkey.toLowerCase();
    return (
      pubkey === self ||
      user.isAgent ||
      (isArchived?.(pubkey) ?? false) ||
      (isBlocked?.(pubkey) ?? false)
    );
  };
  const needle = filter.trim().toLowerCase();
  const matches = (user: UserSearchResult) =>
    needle.length === 0 ||
    personName(user).toLowerCase().includes(needle) ||
    (user.nip05Handle?.toLowerCase().includes(needle) ?? false);

  const byPubkey = new Map<string, UserSearchResult>();
  for (const user of [...users, ...sharedPeople]) {
    const pubkey = user.pubkey.toLowerCase();
    if (!byPubkey.has(pubkey)) byPubkey.set(pubkey, { ...user, pubkey });
  }
  const listed = (ids: ReadonlySet<string>) => {
    const list: UserSearchResult[] = [];
    for (const id of ids) {
      const pubkey = id.toLowerCase();
      const profile = extraProfiles?.[pubkey];
      const user: UserSearchResult | null = profile
        ? {
            pubkey,
            displayName: profile.displayName,
            avatarUrl: profile.avatarUrl,
            nip05Handle: profile.nip05Handle,
            ownerPubkey: profile.ownerPubkey,
            isAgent: profile.isAgent ?? false,
          }
        : (byPubkey.get(pubkey) ?? null);
      if (user && !excluded(user) && matches(user)) list.push(user);
    }
    return list.sort(byName);
  };
  const friendSet = new Set([...friendIds].map((id) => id.toLowerCase()));
  const seen = new Set<string>();
  const inChannels =
    needle.length === 0
      ? []
      : sharedPeople
          .map((user) => ({ ...user, pubkey: user.pubkey.toLowerCase() }))
          .filter((user) => {
            if (seen.has(user.pubkey)) return false;
            seen.add(user.pubkey);
            return !friendSet.has(user.pubkey) && !excluded(user);
          })
          .sort(byName);

  return {
    favorites: listed(favoriteIds),
    friends: listed(friendSet),
    inChannels,
  };
}
