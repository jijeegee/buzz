import * as React from "react";

import { useCommunities } from "@/features/communities/useCommunities";
import { useIdentityQuery } from "@/shared/api/hooks";

import {
  PEOPLE_FAVORITES_LANE,
  useSyncedIdSet,
} from "@/features/sidebar/lib/syncedIdSetLane";

export type PeopleFavorites = {
  favoriteIds: ReadonlySet<string>;
  isFavorite: (pubkey: string) => boolean;
  addFavorite: (pubkey: string) => void;
  removeFavorite: (pubkey: string) => void;
};

const PeopleFavoritesContext = React.createContext<PeopleFavorites | null>(
  null,
);

/**
 * People I starred, synced across my devices like channel stars. One provider
 * per signed-in scope, so the People screen and every profile panel share the
 * same store.
 */
export function PeopleFavoritesProvider({
  children,
  pubkey,
  relayUrl,
}: {
  children: React.ReactNode;
  pubkey: string | undefined;
  relayUrl: string | undefined;
}) {
  const { ids, add, remove } = useSyncedIdSet(
    PEOPLE_FAVORITES_LANE,
    pubkey,
    relayUrl,
  );
  const value = React.useMemo<PeopleFavorites>(
    () => ({
      favoriteIds: ids,
      isFavorite: (target) => ids.has(target.toLowerCase()),
      addFavorite: (target) => add(target.toLowerCase()),
      removeFavorite: (target) => remove(target.toLowerCase()),
    }),
    [add, ids, remove],
  );
  return (
    <PeopleFavoritesContext.Provider value={value}>
      {children}
    </PeopleFavoritesContext.Provider>
  );
}

/** The provider bound to the signed-in account and active community. */
export function ScopedPeopleFavoritesProvider({
  children,
}: {
  children: React.ReactNode;
}) {
  const identityQuery = useIdentityQuery();
  const { activeCommunity } = useCommunities();
  return (
    <PeopleFavoritesProvider
      pubkey={identityQuery.data?.pubkey}
      relayUrl={activeCommunity?.relayUrl}
    >
      {children}
    </PeopleFavoritesProvider>
  );
}

/** Null outside the app shell (tests, standalone screens): hide favorites. */
export function usePeopleFavorites(): PeopleFavorites | null {
  return React.useContext(PeopleFavoritesContext);
}
